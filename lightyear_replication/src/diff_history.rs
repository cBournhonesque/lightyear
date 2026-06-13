//! Receiver-side history support for diff-replicated confirmed state.

use alloc::{collections::BTreeMap, format, vec::Vec};
use core::marker::PhantomData;

use bevy_ecs::component::Component;
use bevy_ecs::error::Result;
use bevy_replicon::shared::replication::diff::{
    Diffable as RepliconDiffable, PatchBatch, PatchIndex,
};
use lightyear_core::prelude::{ConfirmedHistory, ConfirmedState, Tick};

#[derive(Debug, Clone)]
struct PendingPatchMessage<Patch> {
    tick: Tick,
    first_patch_index: PatchIndex,
    patches: Vec<PatchBatch<Patch>>,
}

impl<Patch> PendingPatchMessage<Patch> {
    fn base_cursor(&self) -> Option<PatchIndex> {
        self.first_patch_index.checked_sub(1)
    }

    fn cursor(&self) -> Result<PatchIndex> {
        let offset = self
            .patches
            .len()
            .checked_sub(1)
            .expect("empty patch messages are not queued") as PatchIndex;
        self.first_patch_index.checked_add(offset).ok_or_else(|| {
            format!(
                "patch cursor overflow: first_patch_index={}, patch_count={}",
                self.first_patch_index,
                self.patches.len()
            )
            .into()
        })
    }
}

/// Tracks patch cursors for materialized [`ConfirmedHistory`] entries.
///
/// This is deliberately separate from Replicon's live `PatchBuffer`: prediction
/// and interpolation need to reconstruct historical confirmed values from patch
/// messages without advancing a single live base cursor past older ticks that
/// can still arrive later.
///
/// Cursor model:
/// - A patch cursor is the state after a patch batch has been applied.
///   `None` is the pre-patch state before patch index `0`.
/// - `base_cursor`/`base_tick` are the retained floor. They identify the
///   oldest cursor that can still be used as a base, and the lowest retained
///   confirmed-history tick whose value represents that cursor.
/// - `patch_ticks` maps newer patch cursors to the confirmed-history tick that
///   stores the materialized state for that cursor.
///
/// Invariant:
/// - If `base_tick` is present, it is the lowest retained tick in this receiver.
/// - All keys in `patch_ticks` are strictly newer than `base_cursor`.
/// - Every tick in `patch_ticks` is greater than or equal to `base_tick`.
/// - After pruning to a processed confirmed tick, the newest cursor at or
///   before the prune tick is promoted to `base_cursor`, and `base_tick` is
///   moved to the prune tick so the base can be fetched from `ConfirmedHistory`.
///
/// Lifecycle:
/// 1. The prediction/interpolation `write_fn` receives a [`DiffWire`](bevy_replicon::shared::replication::diff::DiffWire).
/// 2. Snapshots are written directly into [`ConfirmedHistory`] and their cursor
///    is recorded with [`Self::record_cursor`].
/// 3. Patch messages are added with [`Self::queue_patches`]. They may be
///    buffered if their base cursor is not materialized yet.
/// 4. The `write_fn` calls [`Self::take_ready_update`] in a loop. Each ready
///    message fetches its base value from [`ConfirmedHistory`], applies the
///    patch batches, records the new cursor, and returns the materialized value
///    for insertion into [`ConfirmedHistory`].
/// 5. After prediction/interpolation has processed a confirmed server tick,
///    [`Self::clear_before_tick`] promotes the newest usable cursor at that tick
///    to the retained base and drains older cursor/pending state.
#[derive(Component, Debug, Clone)]
pub struct ConfirmedHistoryPatchReceiver<C: RepliconDiffable> {
    base_cursor: Option<PatchIndex>,
    base_tick: Option<Tick>,
    patch_ticks: BTreeMap<PatchIndex, Tick>,
    pending: Vec<PendingPatchMessage<C::Patch>>,
    marker: PhantomData<fn() -> C>,
}

impl<C: RepliconDiffable> Default for ConfirmedHistoryPatchReceiver<C> {
    fn default() -> Self {
        Self {
            base_cursor: None,
            base_tick: None,
            patch_ticks: Default::default(),
            pending: Default::default(),
            marker: PhantomData,
        }
    }
}

impl<C: RepliconDiffable> ConfirmedHistoryPatchReceiver<C> {
    /// Records that confirmed history contains the state for `cursor` at `tick`.
    ///
    /// `cursor` is the patch cursor after the confirmed state was produced:
    /// - `None` for a snapshot with no patch base, i.e. the state before patch `0`.
    /// - `Some(i)` for a snapshot or patch message whose value is the state
    ///   after patch batch `i`.
    ///
    /// Cursors older than the retained base are ignored. Recording `None`
    /// resets the receiver to a full-snapshot base for a new patch stream.
    pub fn record_cursor(&mut self, tick: Tick, cursor: Option<PatchIndex>) {
        match cursor {
            Some(cursor) => {
                if let Some(base_cursor) = self.base_cursor {
                    if cursor < base_cursor {
                        return;
                    }
                    if cursor == base_cursor {
                        if self.base_tick.is_none_or(|base_tick| tick >= base_tick) {
                            self.base_tick = Some(tick);
                        }
                        self.patch_ticks.retain(|index, _| *index > cursor);
                        return;
                    }
                }
                self.patch_ticks.insert(cursor, tick);
            }
            None => {
                if self.base_tick.is_some_and(|base_tick| tick < base_tick) {
                    return;
                }
                self.base_cursor = None;
                self.base_tick = Some(tick);
                self.patch_ticks.clear();
            }
        }
    }

    /// Returns the confirmed tick corresponding to `cursor`.
    pub fn tick_for_cursor(&self, cursor: Option<PatchIndex>) -> Option<Tick> {
        if cursor == self.base_cursor {
            return self.base_tick;
        }
        cursor.and_then(|cursor| self.patch_ticks.get(&cursor).copied())
    }

    /// Returns the newest retained cursor recorded at or before `tick`.
    fn cursor_at_or_before(&self, tick: Tick) -> Option<(Option<PatchIndex>, Tick)> {
        self.base_tick
            .map(|base_tick| (self.base_cursor, base_tick))
            .into_iter()
            .chain(
                self.patch_ticks
                    .iter()
                    .map(|(cursor, cursor_tick)| (Some(*cursor), *cursor_tick)),
            )
            .filter(|(_, cursor_tick)| *cursor_tick <= tick)
            .max_by_key(|(_, cursor_tick)| *cursor_tick)
    }

    /// Shift all stored ticks by `delta`.
    pub fn update_ticks(&mut self, delta: i32) {
        if let Some(tick) = &mut self.base_tick {
            *tick = *tick + delta;
        }
        for tick in self.patch_ticks.values_mut() {
            *tick = *tick + delta;
        }
        for pending in &mut self.pending {
            pending.tick = pending.tick + delta;
        }
    }

    /// Drops cursor and pending-patch state older than `tick`.
    ///
    /// The newest cursor at or before `tick` is promoted to the retained base.
    /// This keeps one usable base for future patch messages without storing
    /// per-unchanged-tick cursor entries.
    pub fn clear_before_tick(&mut self, tick: Tick, history: &ConfirmedHistory<C>) {
        let cursor_at_cut = self.cursor_at_or_before(tick).map(|(cursor, _)| cursor);
        if history
            .get_state_at_or_before(tick)
            .and_then(ConfirmedState::value)
            .is_some()
            && let Some(cursor) = cursor_at_cut
        {
            self.base_cursor = cursor;
            self.base_tick = Some(tick);
        } else {
            self.base_cursor = None;
            self.base_tick = None;
        }
        if let Some(base_cursor) = self.base_cursor {
            self.patch_ticks.retain(|cursor, _| *cursor > base_cursor);
        }
        self.patch_ticks
            .retain(|_, cursor_tick| *cursor_tick >= tick);
        let base_cursor = self.base_cursor;
        self.pending.retain(|pending| {
            pending.tick >= tick
                && pending
                    .cursor()
                    .is_ok_and(|cursor| base_cursor.is_none_or(|base_cursor| cursor > base_cursor))
        });
    }

    /// Queues a patch message for historical materialization.
    ///
    /// `queue_patches` only records the patch message. It does not apply the
    /// patches immediately; callers should invoke [`Self::take_ready_update`]
    /// afterwards, usually in a loop.
    ///
    /// The message is ignored when its final cursor is at or before the retained
    /// base cursor, or when the exact message is already known/pending. Otherwise
    /// it is buffered until its `base_cursor` is available in this receiver.
    ///
    /// The method does not look up `ConfirmedHistory`; readiness is checked by
    /// [`Self::take_ready_update`], which is why out-of-order patch ranges can
    /// be queued before their base state arrives.
    pub fn queue_patches(
        &mut self,
        tick: Tick,
        first_patch_index: PatchIndex,
        patches: Vec<PatchBatch<C::Patch>>,
    ) -> Result<()> {
        if patches.is_empty() {
            return Ok(());
        }
        let pending = PendingPatchMessage {
            tick,
            first_patch_index,
            patches,
        };
        let cursor = pending.cursor()?;
        if self.is_retired_cursor(cursor)
            || self.tick_for_cursor(Some(cursor)).is_some()
            || self.pending.iter().any(|queued| {
                queued.tick == pending.tick
                    && queued.first_patch_index == pending.first_patch_index
                    && queued.cursor().ok() == Some(cursor)
            })
        {
            return Ok(());
        }
        self.pending.push(pending);
        Ok(())
    }

    /// Attempts to materialize one queued patch message from `history`.
    ///
    /// A patch message is ready when the receiver has a tick for its base cursor
    /// and `ConfirmedHistory` can provide the corresponding base value.
    pub fn take_ready_update(&mut self, history: &ConfirmedHistory<C>) -> Result<Option<(Tick, C)>>
    where
        C: Clone,
    {
        let Some((pending_index, mut value)) =
            self.pending.iter().enumerate().find_map(|(i, pending)| {
                let base_tick = self.tick_for_cursor(pending.base_cursor())?;
                if base_tick > pending.tick
                    || self.has_removal_between(history, base_tick, pending.tick)
                {
                    return None;
                }
                history
                    .get_state_at_or_before(base_tick)
                    .and_then(ConfirmedState::value)
                    .cloned()
                    .map(|value| (i, value))
            })
        else {
            return Ok(None);
        };
        let pending = self.pending.remove(pending_index);
        let cursor = pending.cursor()?;
        for batch in &pending.patches {
            for patch in batch {
                value.apply_patch(patch)?;
            }
        }
        self.record_cursor(pending.tick, Some(cursor));
        Ok(Some((pending.tick, value)))
    }

    fn is_retired_cursor(&self, cursor: PatchIndex) -> bool {
        self.base_cursor
            .is_some_and(|base_cursor| cursor <= base_cursor)
    }

    fn has_removal_between(
        &self,
        history: &ConfirmedHistory<C>,
        base_tick: Tick,
        target_tick: Tick,
    ) -> bool {
        (0..history.len()).any(|i| {
            let Some((tick, state)) = history.get_nth_state(i) else {
                return false;
            };
            tick > base_tick && tick <= target_tick && matches!(state, ConfirmedState::Removed)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use bevy_ecs::component::Component;
    use serde::{Deserialize, Serialize};

    #[derive(Component, Clone, Debug, Deserialize, PartialEq, Serialize)]
    struct TestDiffValue(u32);

    impl RepliconDiffable for TestDiffValue {
        type Patch = u32;

        fn apply_patch(&mut self, patch: &Self::Patch) -> Result<()> {
            self.0 = *patch;
            Ok(())
        }
    }

    /// A patch message for `S3 -> S5` is buffered when the receiver has not
    /// materialized `S3` yet.
    ///
    /// On the wire this is `first_patch_index = 4` with batches `4` and `5`:
    /// batch `4` transforms cursor `3` into cursor `4`, and batch `5`
    /// transforms cursor `4` into cursor `5`.
    #[test]
    fn buffers_patch_range_until_base_state_is_available() {
        let mut history = ConfirmedHistory::<TestDiffValue>::default();
        history.insert_present(Tick(0), TestDiffValue(0));

        let mut receiver = ConfirmedHistoryPatchReceiver::<TestDiffValue>::default();
        receiver.record_cursor(Tick(0), Some(0));

        receiver
            .queue_patches(Tick(5), 4, vec![vec![4], vec![5]])
            .unwrap();

        assert_eq!(receiver.take_ready_update(&history).unwrap(), None);
        assert_eq!(receiver.pending.len(), 1);
        assert_eq!(receiver.pending[0].base_cursor(), Some(3));
        assert_eq!(receiver.tick_for_cursor(Some(5)), None);
    }

    /// Once the receiver has pruned through `S3`, a later `S1 -> S3` patch
    /// message is stale. Its final cursor is at the retained base, so it cannot
    /// produce a newer confirmed history value and is ignored.
    #[test]
    fn ignores_patch_range_at_or_before_retained_base_cursor() {
        let mut history = ConfirmedHistory::<TestDiffValue>::default();
        history.insert_present(Tick(0), TestDiffValue(0));
        history.insert_present(Tick(3), TestDiffValue(3));

        let mut receiver = ConfirmedHistoryPatchReceiver::<TestDiffValue>::default();
        receiver.record_cursor(Tick(0), Some(0));
        receiver.record_cursor(Tick(3), Some(3));
        receiver.clear_before_tick(Tick(3), &history);

        receiver
            .queue_patches(Tick(3), 1, vec![vec![1], vec![2], vec![3]])
            .unwrap();

        assert!(receiver.pending.is_empty());
        assert_eq!(receiver.take_ready_update(&history).unwrap(), None);
        assert_eq!(receiver.tick_for_cursor(Some(3)), Some(Tick(3)));
    }

    /// A newer `S3 -> S5` message can arrive before the older `S0 -> S3`
    /// message. The receiver keeps the newer message pending, materializes
    /// `S3` when the older message arrives, and then uses that newly inserted
    /// confirmed history entry as the base for `S5`.
    ///
    /// Here `S0` is represented by cursor `0`, so the older message is encoded
    /// as `first_patch_index = 1` with batches `1`, `2`, and `3`.
    #[test]
    fn older_patch_range_materializes_buffered_newer_range() {
        let mut history = ConfirmedHistory::<TestDiffValue>::default();
        history.insert_present(Tick(0), TestDiffValue(0));

        let mut receiver = ConfirmedHistoryPatchReceiver::<TestDiffValue>::default();
        receiver.record_cursor(Tick(0), Some(0));

        receiver
            .queue_patches(Tick(5), 4, vec![vec![4], vec![5]])
            .unwrap();
        assert_eq!(receiver.take_ready_update(&history).unwrap(), None);

        receiver
            .queue_patches(Tick(3), 1, vec![vec![1], vec![2], vec![3]])
            .unwrap();
        let (tick, value) = receiver.take_ready_update(&history).unwrap().unwrap();
        assert_eq!(tick, Tick(3));
        assert_eq!(value, TestDiffValue(3));
        history.insert_present(tick, value);

        let (tick, value) = receiver.take_ready_update(&history).unwrap().unwrap();
        assert_eq!(tick, Tick(5));
        assert_eq!(value, TestDiffValue(5));
        history.insert_present(tick, value);

        assert_eq!(
            history
                .get_state_at(Tick(3))
                .and_then(ConfirmedState::value)
                .cloned(),
            Some(TestDiffValue(3))
        );
        assert_eq!(
            history
                .get_state_at(Tick(5))
                .and_then(ConfirmedState::value)
                .cloned(),
            Some(TestDiffValue(5))
        );
        assert_eq!(receiver.tick_for_cursor(Some(3)), Some(Tick(3)));
        assert_eq!(receiver.tick_for_cursor(Some(5)), Some(Tick(5)));
    }

    /// Pruning promotes the newest known cursor at or before the processed tick
    /// to the retained base. `base_tick` becomes the lowest retained tick, and
    /// `patch_ticks` only keeps newer cursors that may still be useful.
    #[test]
    fn pruning_promotes_processed_tick_to_retained_base() {
        let mut history = ConfirmedHistory::<TestDiffValue>::default();
        history.insert_present(Tick(0), TestDiffValue(0));
        history.insert_present(Tick(3), TestDiffValue(3));
        history.insert_present(Tick(5), TestDiffValue(5));

        let mut receiver = ConfirmedHistoryPatchReceiver::<TestDiffValue>::default();
        receiver.record_cursor(Tick(0), Some(0));
        receiver.record_cursor(Tick(3), Some(3));
        receiver.record_cursor(Tick(5), Some(5));

        receiver.clear_before_tick(Tick(3), &history);

        assert_eq!(receiver.base_cursor, Some(3));
        assert_eq!(receiver.base_tick, Some(Tick(3)));
        assert_eq!(receiver.tick_for_cursor(None), None);
        assert_eq!(receiver.tick_for_cursor(Some(3)), Some(Tick(3)));
        assert_eq!(receiver.tick_for_cursor(Some(5)), Some(Tick(5)));
        assert!(
            receiver
                .patch_ticks
                .values()
                .all(|tick| *tick >= receiver.base_tick.unwrap())
        );
    }
}
