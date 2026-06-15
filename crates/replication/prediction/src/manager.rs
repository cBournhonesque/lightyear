//! Defines bevy resources needed for Prediction

use bevy_ecs::prelude::*;
use bevy_reflect::Reflect;

use crate::correction::CorrectionPolicy;
use crate::rollback::RollbackState;
use alloc::vec::Vec;
use bevy_ecs::entity::EntityHash;
use bevy_ecs::lifecycle::HookContext;
use bevy_ecs::world::DeferredWorld;
use core::ops::{Deref, DerefMut};
use lightyear_core::prelude::Tick;
use lightyear_replication::prespawn::PreSpawnedReceiver;
use lightyear_sync::prelude::InputTimelineConfig;
use parking_lot::RwLock;

#[derive(Resource)]
pub struct PredictionResource {
    // entity that holds the InputTimeline
    // We use this to avoid having to run a mutable query in component hook
    pub(crate) link_entity: Entity,
}

type EntityHashMap<K, V> = bevy_platform::collections::HashMap<K, V, EntityHash>;

#[derive(Debug, Clone, Copy, Default, Reflect)]
pub enum RollbackMode {
    /// We always rollback, without comparing if there is a match with a recorded history
    ///
    /// - State: rollback on newly received confirmed state, without checking with any predicted history
    ///   In this case there is no need to store a PredictionHistory
    /// - Input: rollback on newly received input, to the latest confirmed input across all remove clients
    ///
    /// It can be useful to always do rollbacks to test that your game can handle the CPU demand of doing the
    /// frequent rollbacks. This also unlocks perf optimizations such as not storing a PredictionHistory.
    Always,
    #[default]
    /// We check if we should rollback by comparing with a previous value.
    Check,
    /// We don't rollback or do any checks.
    /// - State: state rollbacks could be disabled if you're using deterministic replication and only sending inputs
    /// - Input: input rollbacks could be disabled if you're not sending inputs from remote clients
    Disabled,
}

#[derive(Debug, Clone, Copy, Reflect)]
/// The RollbackPolicy defines how we check and trigger rollbacks.
///
/// If State and Input are both enabled, State takes precedence over Input.
/// (if there is mismatch for both, we will rollback from the state mismatch)
pub struct RollbackPolicy {
    pub state: RollbackMode,
    pub input: RollbackMode,
    /// Maximum number of ticks we can rollback to. If we receive some packets that would make us rollback more than
    /// this number of ticks, we just do nothing.
    pub max_rollback_ticks: u16,
}

impl Default for RollbackPolicy {
    fn default() -> Self {
        Self {
            state: RollbackMode::Check,
            input: RollbackMode::Check,
            max_rollback_ticks: 100,
        }
    }
}

impl RollbackPolicy {
    /// Returns true if we don't need to store a prediction history.
    ///
    /// PredictionHistory is not needed if we always rollback on new states
    pub fn no_prediction_history(&self) -> bool {
        !matches!(self.state, RollbackMode::Disabled)
            && matches!(self.input, RollbackMode::Disabled)
    }
}

#[derive(Component, Debug, Reflect)]
#[component(on_insert = PredictionManager::on_insert)]
#[require(InputTimelineConfig)]
#[require(PreSpawnedReceiver)]
#[require(LastConfirmedInput)]
pub struct PredictionManager {
    /// Configuration for how rollbacks are triggered
    pub rollback_policy: RollbackPolicy,
    /// Configuration for smoothing the rollback error over time
    pub correction_policy: CorrectionPolicy,
    /// For input-based rollback: tracks earliest mismatch across remote clients
    pub earliest_mismatch_input: EarliestMismatchedInput,

    #[doc(hidden)]
    pub deterministic_despawn: Vec<(Tick, Entity)>,
    #[doc(hidden)]
    pub deterministic_skip_despawn: Vec<(Tick, Entity)>,
    #[doc(hidden)]
    #[reflect(ignore)]
    pub rollback: RwLock<RollbackState>,
}

/// Store the most recent confirmed input across all remote clients.
#[derive(Component, Debug, Default, Reflect)]
pub struct LastConfirmedInput {
    /// Updated via [`AtomicTick::set_if_lower`] to track the minimum last-confirmed tick
    /// across all remote clients. Reset to a high value each frame by
    /// [`reset_input_rollback_tracker`] so the minimum is computed correctly.
    ///
    /// [`AtomicTick::set_if_lower`]: lightyear_core::tick::AtomicTick::set_if_lower
    /// [`reset_input_rollback_tracker`]: crate::rollback::reset_input_rollback_tracker
    pub tick: lightyear_core::tick::AtomicTick,
    pub received_any_messages: bevy_platform::sync::atomic::AtomicBool,
}

impl LastConfirmedInput {
    pub fn received_input(&self) -> bool {
        self.received_any_messages
            .load(bevy_platform::sync::atomic::Ordering::Relaxed)
    }
}

/// Stores metadata related to state-based prediction.
#[derive(Resource, Clone, Copy, Debug, Default, Reflect)]
pub struct StateRollbackMetadata {
    /// The last confirmed tick where we checked unchanged entities.
    ///
    /// The latest confirmed tick itself is stored in
    /// [`ReplicationCheckpointMap`](lightyear_replication::checkpoint::ReplicationCheckpointMap).
    /// This field only records which completed tick prediction has already
    /// scanned for unchanged-entity rollback checks.
    last_processed_tick: Option<Tick>,

    /// Earliest tick represented by [`Self::mismatch_mask`].
    ///
    /// Once a completed server tick has been processed this is kept equal to
    /// [`Self::last_processed_tick`]. Before that, the first recorded mismatch
    /// anchors the mask so early explicit mismatches are not lost.
    mismatch_history_start: Option<Tick>,

    /// Mismatch bits for the 64 ticks starting at [`Self::mismatch_history_start`].
    ///
    /// Bit 0 corresponds to `mismatch_history_start`, bit 1 to the next tick,
    /// and so on. A set bit means a receive-time confirmed update already
    /// proved that tick mismatched, so we do not need to run another mismatch
    /// comparison for that same tick.
    mismatch_mask: u64,

    /// Set to true if we received any replication message this frame.
    /// Used to trigger `RollbackMode::Always`.
    pub(crate) received_messages_this_frame: bool,

    /// Tick at which an external caller has requested a one-shot rollback.
    ///
    /// Consumed by `check_rollback` regardless of the `rollback_policy.state`
    /// setting — this is an explicit request, not a mismatch-triggered one.
    /// Set via [`StateRollbackMetadata::request_forced_rollback`]. Cleared
    /// when consumed.
    pub(crate) forced_rollback_tick: Option<Tick>,
}

impl StateRollbackMetadata {
    fn mismatch_offset(start: Tick, tick: Tick) -> Option<u32> {
        let delta = tick - start;
        if (0..u64::BITS as i32).contains(&delta) {
            Some(delta as u32)
        } else {
            None
        }
    }

    fn ensure_mismatch_history_start(&mut self, tick: Tick) -> Tick {
        if let Some(start) = self.mismatch_history_start {
            return start;
        }
        let start = self.last_processed_tick.unwrap_or(tick);
        self.mismatch_history_start = Some(start);
        start
    }

    /// Record a receive-time mismatch at `tick`.
    ///
    /// Returns `false` if `tick` is older than the retained mismatch window or
    /// too far ahead to fit in the 64-bit mask.
    pub fn record_mismatch(&mut self, tick: Tick) -> bool {
        let start = self.ensure_mismatch_history_start(tick);
        let Some(offset) = Self::mismatch_offset(start, tick) else {
            return false;
        };
        self.mismatch_mask |= 1_u64 << offset;
        true
    }

    /// Return whether a mismatch has already been recorded for exactly `tick`.
    pub(crate) fn has_mismatch(&self, tick: Tick) -> bool {
        let Some(start) = self.mismatch_history_start else {
            return false;
        };
        let Some(offset) = Self::mismatch_offset(start, tick) else {
            return false;
        };
        self.mismatch_mask & (1_u64 << offset) != 0
    }

    /// Return whether receive-time prediction checks should run for `tick`.
    pub(crate) fn should_check_mismatch_at(&self, tick: Tick) -> bool {
        if self
            .last_processed_tick
            .is_some_and(|last_processed| tick < last_processed)
        {
            return false;
        }
        if let Some(start) = self.mismatch_history_start
            && Self::mismatch_offset(start, tick).is_none()
        {
            return false;
        }
        !self.has_mismatch(tick)
    }

    /// Request a one-shot rollback from `tick`, regardless of the
    /// `rollback_policy.state` mode.
    ///
    /// Intended for scenarios where an external system (e.g. late-join
    /// catch-up) has deposited confirmed state at a specific tick and
    /// needs the simulation to re-run from there. Unlike
    /// [`record_mismatch`], this does not track the earliest across
    /// multiple calls in a frame — the caller is authoritative about the
    /// tick. Subsequent calls within the same frame take the earliest.
    ///
    /// [`record_mismatch`]: StateRollbackMetadata::record_mismatch
    pub fn request_forced_rollback(&mut self, tick: Tick) {
        match self.forced_rollback_tick {
            None => self.forced_rollback_tick = Some(tick),
            Some(existing) if tick < existing => self.forced_rollback_tick = Some(tick),
            _ => {}
        }
    }

    /// Tick at which a one-shot rollback has been requested but not yet
    /// consumed by `check_rollback`. While this is `Some`, prediction
    /// history buffers on the entities targeted by the rollback must not
    /// be mutated (e.g. checksum systems that use destructive reads must
    /// skip), otherwise `prepare_rollback` won't find the restore value.
    pub fn forced_rollback_tick(&self) -> Option<Tick> {
        self.forced_rollback_tick
    }

    /// Reset all connection-scoped rollback metadata.
    pub(crate) fn reset_connection_state(&mut self) {
        *self = Self::default();
    }

    /// Reset the per-frame state tracking.
    /// Note: the mismatch mask is NOT reset here because receive-time mismatch
    /// evidence persists until the completed server tick is processed.
    pub(crate) fn reset_frame_state(&mut self) {
        self.received_messages_this_frame = false;
    }

    /// Clear all retained mismatch evidence.
    pub(crate) fn clear_mismatch_history(&mut self) {
        self.mismatch_mask = 0;
    }

    /// Returns the last confirmed tick that was processed for unchanged entities.
    ///
    /// Used to skip the unchanged-entity rollback check when `last_confirmed_tick`
    /// has not advanced since the last successful check.
    pub fn last_processed_tick(&self) -> Option<Tick> {
        self.last_processed_tick
    }

    /// Update the last processed tick after we've handled confirmed mutate tick advancement.
    ///
    /// Call this only after the unchanged-entity rollback check has actually run
    /// for the tick, not while the confirmed tick is still in the client's future.
    pub fn set_last_processed_tick(&mut self, tick: Tick) {
        match self.mismatch_history_start {
            None => self.mismatch_history_start = Some(tick),
            Some(start) => {
                let delta = tick - start;
                if delta > 0 {
                    if delta >= u64::BITS as i32 {
                        self.mismatch_mask = 0;
                    } else {
                        self.mismatch_mask >>= delta as u32;
                    }
                    self.mismatch_history_start = Some(tick);
                } else if delta < 0 {
                    let delta = -delta;
                    if delta >= u64::BITS as i32 {
                        self.mismatch_mask = 0;
                    } else {
                        self.mismatch_mask <<= delta as u32;
                    }
                    self.mismatch_history_start = Some(tick);
                }
            }
        }
        self.last_processed_tick = Some(tick);
    }

    /// Check if the completed mutate tick has advanced since we last processed it.
    ///
    /// If this returns false, `check_rollback` can skip the unchanged-entity
    /// rollback scan because the current `last_confirmed_tick` was already
    /// handled on an earlier frame.
    pub fn has_confirmed_tick_advanced(&self, current_tick: Tick) -> bool {
        match self.last_processed_tick {
            None => true, // First time, always process
            Some(last) => current_tick > last,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mismatch_history_tracks_exact_ticks() {
        let mut metadata = StateRollbackMetadata::default();
        metadata.set_last_processed_tick(Tick(10));
        metadata.record_mismatch(Tick(12));

        assert!(!metadata.has_mismatch(Tick(11)));
        assert!(metadata.has_mismatch(Tick(12)));
        assert!(!metadata.should_check_mismatch_at(Tick(9)));
        assert!(!metadata.should_check_mismatch_at(Tick(12)));
        assert!(metadata.should_check_mismatch_at(Tick(13)));

        metadata.set_last_processed_tick(Tick(11));
        assert!(metadata.has_mismatch(Tick(12)));

        metadata.set_last_processed_tick(Tick(12));
        assert!(metadata.has_mismatch(Tick(12)));

        metadata.clear_mismatch_history();
        assert!(!metadata.has_mismatch(Tick(12)));
    }

    #[test]
    fn confirmed_tick_advancement_uses_last_processed_tick() {
        let mut metadata = StateRollbackMetadata::default();
        assert!(metadata.has_confirmed_tick_advanced(Tick(10)));

        metadata.set_last_processed_tick(Tick(10));
        assert!(!metadata.has_confirmed_tick_advanced(Tick(10)));
        assert!(!metadata.has_confirmed_tick_advanced(Tick(9)));
        assert!(metadata.has_confirmed_tick_advanced(Tick(11)));
    }

    #[test]
    fn server_mutate_last_tick_can_be_newer_than_latest_complete_tick() {
        use bevy_replicon::client::server_mutate_ticks::ServerMutateTicks;
        use bevy_replicon::prelude::RepliconTick;

        let complete_tick = RepliconTick::new(9);
        let incomplete_tick = RepliconTick::new(10);

        let mut server_mutate_ticks = ServerMutateTicks::default();
        assert!(server_mutate_ticks.confirm(complete_tick, 1));
        assert!(!server_mutate_ticks.confirm(incomplete_tick, 2));

        assert_eq!(server_mutate_ticks.last_tick(), incomplete_tick);
        assert!(server_mutate_ticks.contains(complete_tick));
        assert!(!server_mutate_ticks.contains(incomplete_tick));
    }

    #[test]
    fn mismatch_history_keeps_multiple_exact_ticks() {
        let mut metadata = StateRollbackMetadata::default();
        metadata.set_last_processed_tick(Tick(10));

        metadata.record_mismatch(Tick(12));
        metadata.record_mismatch(Tick(14));
        metadata.record_mismatch(Tick(10));

        assert!(metadata.has_mismatch(Tick(10)));
        assert!(!metadata.has_mismatch(Tick(11)));
        assert!(metadata.has_mismatch(Tick(12)));
        assert!(!metadata.has_mismatch(Tick(13)));
        assert!(metadata.has_mismatch(Tick(14)));

        metadata.set_last_processed_tick(Tick(13));
        assert!(!metadata.has_mismatch(Tick(12)));
        assert!(metadata.has_mismatch(Tick(14)));
    }

    #[test]
    fn mismatch_history_reanchors_to_first_processed_tick() {
        let mut metadata = StateRollbackMetadata::default();

        metadata.record_mismatch(Tick(12));
        metadata.set_last_processed_tick(Tick(10));

        assert_eq!(metadata.mismatch_history_start, Some(Tick(10)));
        assert!(metadata.has_mismatch(Tick(12)));
    }
}

/// Store the earliest mismatched input across all remote clients.
#[derive(Debug, Reflect)]
pub struct EarliestMismatchedInput {
    /// Initialized to `Tick::MAX` so the first [`AtomicTick::set_if_lower`] call wins.
    /// Updated via [`AtomicTick::set_if_lower`] to track the minimum mismatch tick
    /// across all remote clients.
    ///
    /// [`AtomicTick::set_if_lower`]: lightyear_core::tick::AtomicTick::set_if_lower
    pub tick: lightyear_core::tick::AtomicTick,
    pub has_mismatches: bevy_platform::sync::atomic::AtomicBool,
}

impl Default for EarliestMismatchedInput {
    fn default() -> Self {
        Self {
            tick: lightyear_core::tick::AtomicTick::new_max(),
            has_mismatches: bevy_platform::sync::atomic::AtomicBool::new(false),
        }
    }
}

impl EarliestMismatchedInput {
    pub fn has_mismatches(&self) -> bool {
        self.has_mismatches
            .load(bevy_platform::sync::atomic::Ordering::Relaxed)
    }
}

impl Default for PredictionManager {
    fn default() -> Self {
        Self {
            rollback_policy: RollbackPolicy::default(),
            correction_policy: CorrectionPolicy::default(),
            earliest_mismatch_input: EarliestMismatchedInput::default(),
            deterministic_skip_despawn: Vec::default(),
            deterministic_despawn: Vec::default(),
            rollback: RwLock::new(RollbackState::Default),
        }
    }
}

impl PredictionManager {
    fn on_insert(mut deferred: DeferredWorld, context: HookContext) {
        let entity = context.entity;
        deferred.commands().queue(move |world: &mut World| {
            world.insert_resource(PredictionResource {
                link_entity: entity,
            });
        })
    }
}

// SAFETY: We never use UnsafeCell to mutate the predicted_entity_map, so it's safe to send and sync
unsafe impl Send for PredictionManager {}
unsafe impl Sync for PredictionManager {}

impl PredictionManager {
    /// Returns true if we are currently in a rollback state
    pub fn is_rollback(&self) -> bool {
        match *self.rollback.read().deref() {
            RollbackState::RollbackStart { .. } => true,
            RollbackState::Default => false,
        }
    }

    /// Get the current rollback tick
    pub fn get_rollback_start_tick(&self) -> Option<Tick> {
        match *self.rollback.read().deref() {
            RollbackState::RollbackStart(start_tick) => Some(start_tick),
            RollbackState::Default => None,
        }
    }

    /// Set the rollback state back to non-rollback
    pub fn set_non_rollback(&self) {
        *self.rollback.write().deref_mut() = RollbackState::Default;
    }

    /// Set the rollback state to `ShouldRollback` with the given tick.
    pub fn set_rollback_tick(&self, tick: Tick) {
        *self.rollback.write().deref_mut() = RollbackState::RollbackStart(tick)
    }
}
