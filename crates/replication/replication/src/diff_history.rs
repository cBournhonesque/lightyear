//! Receiver-side history support for diff-replicated confirmed state.

use alloc::{format, vec::Vec};
use core::marker::PhantomData;

use bevy_ecs::error::Result;
use bevy_replicon::shared::replication::diff::{
    Diffable as RepliconDiffable, diff_index::DiffIndex,
};
use lightyear_core::prelude::{ConfirmedHistory, HistoryState, Tick};

#[derive(Debug, Clone)]
struct PendingDiffMessage<Diff> {
    tick: Tick,
    first_diff_index: DiffIndex,
    diffs: Vec<Diff>,
}

impl<Diff> PendingDiffMessage<Diff> {
    fn base_cursor(&self) -> DiffIndex {
        self.first_diff_index - 1
    }

    fn cursor(&self) -> Result<DiffIndex> {
        let offset = self
            .diffs
            .len()
            .checked_sub(1)
            .expect("empty diff messages are not queued");
        let offset = u16::try_from(offset).map_err(|_| {
            format!(
                "too many diffs in one message: first_diff_index={}, diff_count={}",
                self.first_diff_index.get(),
                self.diffs.len()
            )
        })?;
        Ok(self.first_diff_index + offset)
    }
}

/// Tracks diff cursors for materialized [`ConfirmedHistory`] entries.
///
/// This is deliberately separate from Replicon's live `DiffBuffer`: prediction
/// and interpolation need to reconstruct historical confirmed values from diff
/// messages without advancing a single live base cursor past older ticks that
/// can still arrive later.
///
/// Cursor model:
/// - A diff cursor is the state after a diff has been applied.
///   `None` is the pre-diff state before diff index `0`.
/// - `base_cursor`/`base_tick` are the retained floor. They identify the
///   oldest cursor that can still be used as a base, and the lowest retained
///   confirmed-history tick whose value represents that cursor.
/// - `diff_ticks` maps newer diff cursors to the confirmed-history tick that
///   stores the materialized state for that cursor.
///
/// Invariant:
/// - If `base_tick` is present, it is the lowest retained tick in this receiver.
/// - All keys in `diff_ticks` are strictly newer than `base_cursor`.
/// - Every tick in `diff_ticks` is greater than or equal to `base_tick`.
/// - After pruning to a processed confirmed tick, the newest cursor at or
///   before the prune tick is promoted to `base_cursor`, and `base_tick` is
///   moved to the prune tick so the base can be fetched from `ConfirmedHistory`.
///
/// Lifecycle:
/// 1. The prediction/interpolation `write_fn` receives a [`ComponentDelta`](bevy_replicon::shared::replication::diff::ComponentDelta).
/// 2. Snapshots are written directly into [`ConfirmedHistory`] and their cursor
///    is recorded with [`Self::record_cursor`].
/// 3. Diff messages are added with [`Self::queue_diffs`]. They may be
///    buffered if their base cursor is not materialized yet.
/// 4. The `write_fn` calls [`Self::take_ready_update`] in a loop. Each ready
///    message fetches its base value from [`ConfirmedHistory`], applies the
///    diffs, records the new cursor, and returns the materialized value
///    for insertion into [`ConfirmedHistory`].
/// 5. After prediction/interpolation has processed a confirmed server tick,
///    [`Self::clear_before_tick`] promotes the newest usable cursor at that tick
///    to the retained base and drains older cursor/pending state.
#[derive(Debug, Clone)]
pub struct HistoryDiffReceiver<C: RepliconDiffable> {
    base_cursor: Option<DiffIndex>,
    base_tick: Option<Tick>,
    diff_ticks: Vec<(DiffIndex, Tick)>,
    pending: Vec<PendingDiffMessage<C::Diff>>,
    marker: PhantomData<fn() -> C>,
}

impl<C: RepliconDiffable> Default for HistoryDiffReceiver<C> {
    fn default() -> Self {
        Self {
            base_cursor: None,
            base_tick: None,
            diff_ticks: Default::default(),
            pending: Default::default(),
            marker: PhantomData,
        }
    }
}

impl<C: RepliconDiffable> HistoryDiffReceiver<C> {
    /// Records that confirmed history contains the state for `cursor` at `tick`.
    ///
    /// `cursor` is the diff cursor after the confirmed state was produced:
    /// - `None` for a snapshot with no diff base, i.e. the state before diff `0`.
    /// - `Some(i)` for a snapshot or diff message whose value is the state
    ///   after diff batch `i`.
    ///
    /// Cursors older than the retained base are ignored. Recording `None`
    /// resets the receiver to a full-snapshot base for a new diff stream.
    pub fn record_cursor(&mut self, tick: Tick, cursor: Option<DiffIndex>) {
        match cursor {
            Some(cursor) => {
                if let Some(base_cursor) = self.base_cursor {
                    if cursor != base_cursor && !cursor.is_newer_than(base_cursor) {
                        return;
                    }
                    if cursor == base_cursor {
                        if self.base_tick.is_none_or(|base_tick| tick >= base_tick) {
                            self.base_tick = Some(tick);
                        }
                        self.diff_ticks
                            .retain(|(index, _)| index.is_newer_than(cursor));
                        self.pending.retain(|pending| {
                            pending
                                .cursor()
                                .is_ok_and(|pending_cursor| pending_cursor.is_newer_than(cursor))
                        });
                        return;
                    }
                }
                if let Some((_, cursor_tick)) = self
                    .diff_ticks
                    .iter_mut()
                    .find(|(known_cursor, _)| *known_cursor == cursor)
                {
                    *cursor_tick = tick;
                } else {
                    self.diff_ticks.push((cursor, tick));
                }
                self.pending.retain(|pending| {
                    pending
                        .cursor()
                        .is_ok_and(|pending_cursor| pending_cursor.is_newer_than(cursor))
                });
            }
            None => {
                if self.base_tick.is_some_and(|base_tick| tick < base_tick) {
                    return;
                }
                self.base_cursor = None;
                self.base_tick = Some(tick);
                self.diff_ticks.clear();
                self.pending.clear();
            }
        }
    }

    /// Returns the confirmed tick corresponding to `cursor`.
    pub fn tick_for_cursor(&self, cursor: Option<DiffIndex>) -> Option<Tick> {
        if cursor == self.base_cursor {
            return self.base_tick;
        }
        cursor.and_then(|cursor| {
            self.diff_ticks
                .iter()
                .find_map(|(known_cursor, tick)| (*known_cursor == cursor).then_some(*tick))
        })
    }

    /// Returns the newest retained cursor recorded at or before `tick`.
    fn cursor_at_or_before(&self, tick: Tick) -> Option<(Option<DiffIndex>, Tick)> {
        self.base_tick
            .map(|base_tick| (self.base_cursor, base_tick))
            .into_iter()
            .chain(
                self.diff_ticks
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
        for (_, tick) in &mut self.diff_ticks {
            *tick = *tick + delta;
        }
        for pending in &mut self.pending {
            pending.tick = pending.tick + delta;
        }
    }

    /// Drops cursor and pending-diff state older than `tick`.
    ///
    /// The newest cursor at or before `tick` is promoted to the retained base.
    /// This keeps one usable base for future diff messages without storing
    /// per-unchanged-tick cursor entries.
    pub fn clear_before_tick(&mut self, tick: Tick, history: &ConfirmedHistory<C>) {
        if self.has_pending_diffs() {
            return;
        }
        if self.base_tick.is_some_and(|base_tick| tick < base_tick) {
            return;
        }

        let cursor_at_cut = self.cursor_at_or_before(tick).map(|(cursor, _)| cursor);
        if history
            .get_state_at_or_before(tick)
            .and_then(HistoryState::value)
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
            self.diff_ticks
                .retain(|(cursor, _)| cursor.is_newer_than(base_cursor));
        }
        self.diff_ticks
            .retain(|(_, cursor_tick)| *cursor_tick >= tick);
        let base_cursor = self.base_cursor;
        self.pending.retain(|pending| {
            pending.cursor().is_ok_and(|cursor| {
                base_cursor.is_none_or(|base_cursor| cursor.is_newer_than(base_cursor))
            })
        });
    }

    /// Returns true while one or more diff messages are waiting for a base
    /// cursor to be materialized in confirmed history.
    pub fn has_pending_diffs(&self) -> bool {
        !self.pending.is_empty()
    }

    /// Returns true when a diff message for `tick` is waiting for a base
    /// cursor to be materialized in confirmed history.
    pub fn has_pending_diff_at_tick(&self, tick: Tick) -> bool {
        self.pending.iter().any(|pending| pending.tick == tick)
    }

    /// Queues a diff message for historical materialization.
    ///
    /// `queue_diffs` only records the diff message. It does not apply the
    /// diffs immediately; callers should invoke [`Self::take_ready_update`]
    /// afterwards, usually in a loop.
    ///
    /// The message is ignored when its final cursor is at or before the retained
    /// base cursor, or when the exact message is already known/pending. Otherwise
    /// it is buffered until its `base_cursor` is available in this receiver.
    ///
    /// The method does not look up `ConfirmedHistory`; readiness is checked by
    /// [`Self::take_ready_update`], which is why out-of-order diff ranges can
    /// be queued before their base state arrives.
    pub fn queue_diffs(
        &mut self,
        tick: Tick,
        first_diff_index: DiffIndex,
        diffs: Vec<C::Diff>,
    ) -> Result<()> {
        if diffs.is_empty() {
            return Ok(());
        }
        let mut pending = PendingDiffMessage {
            tick,
            first_diff_index,
            diffs,
        };
        let cursor = pending.cursor()?;
        if self.is_retired_cursor(cursor) || self.tick_for_cursor(Some(cursor)).is_some() {
            return Ok(());
        }

        if let Some(base_cursor) = self.base_cursor
            && base_cursor.is_newer_than(pending.base_cursor())
        {
            let retained_first_diff = base_cursor + 1;
            let drop_count = retained_first_diff.distance_after(pending.first_diff_index) as usize;
            if drop_count >= pending.diffs.len() {
                return Ok(());
            }
            pending.diffs.drain(0..drop_count);
            pending.first_diff_index = retained_first_diff;
        }

        let cursor = pending.cursor()?;
        if self.tick_for_cursor(Some(cursor)).is_some()
            || self.pending.iter().any(|queued| {
                queued.tick == pending.tick
                    && queued.first_diff_index == pending.first_diff_index
                    && queued.cursor().ok() == Some(cursor)
            })
        {
            return Ok(());
        }
        self.pending.push(pending);
        Ok(())
    }

    /// Queues a Replicon diff message encoded as the final cursor plus all
    /// diffs needed to reach it.
    pub fn queue_diff(&mut self, tick: Tick, index: DiffIndex, diffs: Vec<C::Diff>) -> Result<()> {
        if diffs.is_empty() {
            return Ok(());
        }
        let offset = diffs.len() - 1;
        let offset = u16::try_from(offset).map_err(|_| {
            format!(
                "too many diffs in one message: index={}, diff_count={}",
                index.get(),
                diffs.len()
            )
        })?;
        // DiffIndex arithmetic wraps, so this also handles ranges that cross
        // u16::MAX, e.g. final index 3 with 10 diffs starts at 65530.
        let first_diff_index = index - offset;
        self.queue_diffs(tick, first_diff_index, diffs)
    }

    /// Attempts to materialize one queued diff message from `history`.
    ///
    /// A diff message is ready when the receiver has a tick for its base cursor
    /// and `ConfirmedHistory` can provide the corresponding base value.
    pub fn take_ready_update(&mut self, history: &ConfirmedHistory<C>) -> Result<Option<(Tick, C)>>
    where
        C: Clone,
    {
        let Some((pending_index, mut value)) =
            self.pending.iter().enumerate().find_map(|(i, pending)| {
                let base_tick = self.tick_for_cursor(Some(pending.base_cursor()))?;
                if base_tick > pending.tick
                    || self.has_removal_between(history, base_tick, pending.tick)
                {
                    return None;
                }
                history
                    .get_state_at_or_before(base_tick)
                    .and_then(HistoryState::value)
                    .cloned()
                    .map(|value| (i, value))
            })
        else {
            return Ok(None);
        };
        let pending = self.pending.remove(pending_index);
        let cursor = pending.cursor()?;
        for diff in &pending.diffs {
            value.apply_diff(diff)?;
        }
        self.record_cursor(pending.tick, Some(cursor));
        Ok(Some((pending.tick, value)))
    }

    fn is_retired_cursor(&self, cursor: DiffIndex) -> bool {
        self.base_cursor
            .is_some_and(|base_cursor| cursor == base_cursor || !cursor.is_newer_than(base_cursor))
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
            tick > base_tick && tick <= target_tick && matches!(state, HistoryState::Removed)
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
        type Diff = u32;

        fn apply_diff(&mut self, diff: &Self::Diff) -> Result<()> {
            self.0 = *diff;
            Ok(())
        }
    }

    fn idx(value: u16) -> DiffIndex {
        DiffIndex::new(value)
    }

    /// A diff message for `S3 -> S5` is buffered when the receiver has not
    /// materialized `S3` yet.
    ///
    /// On the wire this is `first_diff_index = 4` with batches `4` and `5`:
    /// batch `4` transforms cursor `3` into cursor `4`, and batch `5`
    /// transforms cursor `4` into cursor `5`.
    #[test]
    fn buffers_diff_range_until_base_state_is_available() {
        let mut history = ConfirmedHistory::<TestDiffValue>::default();
        history.insert_present(Tick(0), TestDiffValue(0));

        let mut receiver = HistoryDiffReceiver::<TestDiffValue>::default();
        receiver.record_cursor(Tick(0), Some(idx(0)));

        receiver.queue_diffs(Tick(5), idx(4), vec![4, 5]).unwrap();

        assert_eq!(receiver.take_ready_update(&history).unwrap(), None);
        assert_eq!(receiver.pending.len(), 1);
        assert_eq!(receiver.pending[0].base_cursor(), idx(3));
        assert_eq!(receiver.tick_for_cursor(Some(idx(5))), None);
    }

    /// Once the receiver has pruned through `S3`, a later `S1 -> S3` diff
    /// message is stale. Its final cursor is at the retained base, so it cannot
    /// produce a newer confirmed history value and is ignored.
    #[test]
    fn ignores_diff_range_at_or_before_retained_base_cursor() {
        let mut history = ConfirmedHistory::<TestDiffValue>::default();
        history.insert_present(Tick(0), TestDiffValue(0));
        history.insert_present(Tick(3), TestDiffValue(3));

        let mut receiver = HistoryDiffReceiver::<TestDiffValue>::default();
        receiver.record_cursor(Tick(0), Some(idx(0)));
        receiver.record_cursor(Tick(3), Some(idx(3)));
        receiver.clear_before_tick(Tick(3), &history);

        receiver
            .queue_diffs(Tick(3), idx(1), vec![1, 2, 3])
            .unwrap();

        assert!(receiver.pending.is_empty());
        assert_eq!(receiver.take_ready_update(&history).unwrap(), None);
        assert_eq!(receiver.tick_for_cursor(Some(idx(3))), Some(Tick(3)));
    }

    /// A newer `S3 -> S5` message can arrive before the older `S0 -> S3`
    /// message. The receiver keeps the newer message pending, materializes
    /// `S3` when the older message arrives, and then uses that newly inserted
    /// confirmed history entry as the base for `S5`.
    ///
    /// Here `S0` is represented by cursor `0`, so the older message is encoded
    /// as `first_diff_index = 1` with batches `1`, `2`, and `3`.
    #[test]
    fn older_diff_range_materializes_buffered_newer_range() {
        let mut history = ConfirmedHistory::<TestDiffValue>::default();
        history.insert_present(Tick(0), TestDiffValue(0));

        let mut receiver = HistoryDiffReceiver::<TestDiffValue>::default();
        receiver.record_cursor(Tick(0), Some(idx(0)));

        receiver.queue_diffs(Tick(5), idx(4), vec![4, 5]).unwrap();
        assert_eq!(receiver.take_ready_update(&history).unwrap(), None);

        receiver
            .queue_diffs(Tick(3), idx(1), vec![1, 2, 3])
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
                .and_then(HistoryState::value)
                .cloned(),
            Some(TestDiffValue(3))
        );
        assert_eq!(
            history
                .get_state_at(Tick(5))
                .and_then(HistoryState::value)
                .cloned(),
            Some(TestDiffValue(5))
        );
        assert_eq!(receiver.tick_for_cursor(Some(idx(3))), Some(Tick(3)));
        assert_eq!(receiver.tick_for_cursor(Some(idx(5))), Some(Tick(5)));
    }

    /// Receiver cleanup must not retire bases or discard pending diff messages
    /// while a newer diff range is waiting for an older base. Otherwise an
    /// unchanged history anchor can be carried forward and the later base
    /// message can no longer materialize the missing historical state.
    #[test]
    fn pending_diff_range_blocks_receiver_cleanup_until_materialized() {
        let mut history = ConfirmedHistory::<TestDiffValue>::default();
        history.insert_present(Tick(0), TestDiffValue(0));

        let mut receiver = HistoryDiffReceiver::<TestDiffValue>::default();
        receiver.record_cursor(Tick(0), Some(idx(0)));

        receiver.queue_diffs(Tick(5), idx(4), vec![4, 5]).unwrap();
        receiver.clear_before_tick(Tick(6), &history);

        assert_eq!(receiver.pending.len(), 1);
        assert_eq!(receiver.tick_for_cursor(Some(idx(0))), Some(Tick(0)));

        receiver
            .queue_diffs(Tick(3), idx(1), vec![1, 2, 3])
            .unwrap();
        let (tick, value) = receiver.take_ready_update(&history).unwrap().unwrap();
        assert_eq!((tick, value.clone()), (Tick(3), TestDiffValue(3)));
        history.insert_present(tick, value);

        let (tick, value) = receiver.take_ready_update(&history).unwrap().unwrap();
        assert_eq!((tick, value), (Tick(5), TestDiffValue(5)));
    }

    /// Unchanged anchors are same-as-precedent markers, so they can safely be
    /// inserted beyond a pending older diff range. When that older range
    /// materializes, later unchanged anchors inherit the newly inserted value.
    #[test]
    fn older_diff_range_updates_later_unchanged_anchor_when_materialized() {
        let mut history = ConfirmedHistory::<TestDiffValue>::default();
        history.insert_present(Tick(0), TestDiffValue(0));

        let mut receiver = HistoryDiffReceiver::<TestDiffValue>::default();
        receiver.record_cursor(Tick(0), Some(idx(0)));

        receiver.queue_diffs(Tick(5), idx(4), vec![4, 5]).unwrap();
        assert_eq!(receiver.take_ready_update(&history).unwrap(), None);
        assert_eq!(history.push_unchanged(Tick(6)), Some(Tick(0)));

        receiver
            .queue_diffs(Tick(3), idx(1), vec![1, 2, 3])
            .unwrap();
        while let Some((tick, value)) = receiver.take_ready_update(&history).unwrap() {
            history.insert_present(tick, value);
        }

        assert_eq!(
            history
                .get_state_at(Tick(6))
                .and_then(HistoryState::value)
                .cloned(),
            Some(TestDiffValue(5))
        );
    }

    /// If a late message overlaps the retained base cursor, the receiver can
    /// apply the suffix of that message from the retained base instead of
    /// waiting forever for a retired cursor.
    #[test]
    fn overlapping_diff_range_is_trimmed_to_retained_base() {
        let mut history = ConfirmedHistory::<TestDiffValue>::default();
        history.insert_present(Tick(0), TestDiffValue(0));
        history.insert_present(Tick(3), TestDiffValue(3));

        let mut receiver = HistoryDiffReceiver::<TestDiffValue>::default();
        receiver.record_cursor(Tick(0), Some(idx(0)));
        receiver.record_cursor(Tick(3), Some(idx(3)));
        receiver.clear_before_tick(Tick(3), &history);

        receiver
            .queue_diffs(Tick(5), idx(1), vec![1, 2, 3, 4, 5])
            .unwrap();

        assert_eq!(receiver.pending.len(), 1);
        assert_eq!(receiver.pending[0].first_diff_index, idx(4));
        assert_eq!(receiver.pending[0].base_cursor(), idx(3));

        let (tick, value) = receiver.take_ready_update(&history).unwrap().unwrap();
        assert_eq!((tick, value), (Tick(5), TestDiffValue(5)));
        assert!(receiver.pending.is_empty());
        assert_eq!(receiver.tick_for_cursor(Some(idx(5))), Some(Tick(5)));
    }

    /// Replicon's diff indexes wrap at u16::MAX. A message ending at cursor 3
    /// with 10 diffs starts at cursor 65530 and uses 65529 as its base.
    #[test]
    fn queue_diff_handles_wrapped_diff_indexes() {
        let mut history = ConfirmedHistory::<TestDiffValue>::default();
        history.insert_present(Tick(0), TestDiffValue(0));

        let mut receiver = HistoryDiffReceiver::<TestDiffValue>::default();
        receiver.record_cursor(Tick(0), Some(idx(u16::MAX - 6)));

        receiver
            .queue_diff(Tick(10), idx(3), vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10])
            .unwrap();

        let (tick, value) = receiver.take_ready_update(&history).unwrap().unwrap();
        assert_eq!((tick, value), (Tick(10), TestDiffValue(10)));
        assert!(receiver.pending.is_empty());
        assert_eq!(receiver.tick_for_cursor(Some(idx(3))), Some(Tick(10)));
    }

    /// Pruning promotes the newest known cursor at or before the processed tick
    /// to the retained base. `base_tick` becomes the lowest retained tick, and
    /// `diff_ticks` only keeps newer cursors that may still be useful.
    #[test]
    fn pruning_promotes_processed_tick_to_retained_base() {
        let mut history = ConfirmedHistory::<TestDiffValue>::default();
        history.insert_present(Tick(0), TestDiffValue(0));
        history.insert_present(Tick(3), TestDiffValue(3));
        history.insert_present(Tick(5), TestDiffValue(5));

        let mut receiver = HistoryDiffReceiver::<TestDiffValue>::default();
        receiver.record_cursor(Tick(0), Some(idx(0)));
        receiver.record_cursor(Tick(3), Some(idx(3)));
        receiver.record_cursor(Tick(5), Some(idx(5)));

        receiver.clear_before_tick(Tick(3), &history);

        assert_eq!(receiver.base_cursor, Some(idx(3)));
        assert_eq!(receiver.base_tick, Some(Tick(3)));
        assert_eq!(receiver.tick_for_cursor(None), None);
        assert_eq!(receiver.tick_for_cursor(Some(idx(3))), Some(Tick(3)));
        assert_eq!(receiver.tick_for_cursor(Some(idx(5))), Some(Tick(5)));
        assert!(
            receiver
                .diff_ticks
                .iter()
                .all(|(_, tick)| *tick >= receiver.base_tick.unwrap())
        );
    }

    /// Cleanup can be called with an older completed tick after a newer
    /// snapshot was written in the same frame. That must not wipe the retained
    /// base cursor, or the next diff range will be unable to find its base.
    #[test]
    fn pruning_older_than_retained_base_is_noop() {
        let mut history = ConfirmedHistory::<TestDiffValue>::default();
        history.insert_present(Tick(126), TestDiffValue(0));
        history.insert_present(Tick(128), TestDiffValue(0));

        let mut receiver = HistoryDiffReceiver::<TestDiffValue>::default();
        receiver.record_cursor(Tick(128), Some(idx(0)));
        receiver.clear_before_tick(Tick(128), &history);

        assert_eq!(
            (receiver.base_cursor, receiver.base_tick),
            (Some(idx(0)), Some(Tick(128)))
        );

        receiver.clear_before_tick(Tick(126), &history);

        assert_eq!(
            (receiver.base_cursor, receiver.base_tick),
            (Some(idx(0)), Some(Tick(128)))
        );
        assert_eq!(receiver.tick_for_cursor(Some(idx(0))), Some(Tick(128)));
    }
}
