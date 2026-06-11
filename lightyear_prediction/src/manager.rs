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
    /// Updated via [`set_if_lower`] to track the minimum last-confirmed tick
    /// across all remote clients. Reset to a high value each frame by
    /// [`reset_input_rollback_tracker`] so the minimum is computed correctly.
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
///
/// Key invariant: `last_confirmed_tick = T` guarantees that for all entities,
/// we have complete information at tick T:
/// - Entities that received an update at T: their confirmed value is in the message
/// - Entities that didn't receive an update: their value at T = their last confirmed value
///   (because if a message was lost/in-flight, the server would resend on the next tick)
#[derive(Resource, Clone, Copy, Debug, Default, Reflect)]
pub struct StateRollbackMetadata {
    /// The latest authoritative tick for which all mutate messages were received.
    last_confirmed_tick: Option<Tick>,

    /// The last confirmed tick where we checked unchanged entities.
    ///
    /// This is separate from `last_confirmed_tick`: a confirmed tick can stay
    /// unchanged across many frames, and `check_rollback` only needs to scan
    /// unchanged entities once per completed tick. If the confirmed tick is in
    /// the client's future, it is not marked processed yet so the check can be
    /// retried once local prediction history reaches that tick.
    last_processed_tick: Option<Tick>,

    /// The earliest tick where we detected a mismatch this frame.
    /// If set, we will trigger a rollback from this tick.
    pub(crate) earliest_mismatch_tick: Option<Tick>,

    /// Set to true if we received any replication message this frame.
    /// Used to trigger `RollbackMode::Always`.
    pub(crate) received_messages_this_frame: bool,

    /// Set to true if we detected a mismatch and should rollback (for RollbackMode::Check)
    pub(crate) should_rollback: bool,

    /// Tick at which an external caller has requested a one-shot rollback.
    ///
    /// Consumed by `check_rollback` regardless of the `rollback_policy.state`
    /// setting — this is an explicit request, not a mismatch-triggered one.
    /// Set via [`StateRollbackMetadata::request_forced_rollback`]. Cleared
    /// when consumed.
    pub(crate) forced_rollback_tick: Option<Tick>,
}

impl StateRollbackMetadata {
    /// Record a mismatch at the given tick.
    /// The rollback will start from the earliest mismatch tick.
    pub fn record_mismatch(&mut self, tick: Tick) {
        self.should_rollback = true;
        match self.earliest_mismatch_tick {
            None => self.earliest_mismatch_tick = Some(tick),
            Some(existing) if tick < existing => self.earliest_mismatch_tick = Some(tick),
            _ => {}
        }
    }

    /// Returns true when a mismatch earlier than `tick` is already pending.
    pub(crate) fn has_mismatch_before(&self, tick: Tick) -> bool {
        self.should_rollback
            && self
                .earliest_mismatch_tick
                .is_some_and(|mismatch_tick| mismatch_tick < tick)
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

    /// Latest authoritative tick for which all mutate messages were received.
    pub fn last_confirmed_tick(&self) -> Option<Tick> {
        self.last_confirmed_tick
    }

    /// Record a newly completed mutate tick.
    pub fn record_last_confirmed_tick(&mut self, tick: Tick) {
        match self.last_confirmed_tick {
            None => self.last_confirmed_tick = Some(tick),
            Some(existing) if tick > existing => self.last_confirmed_tick = Some(tick),
            _ => {}
        }
    }

    /// Reset all connection-scoped rollback metadata.
    pub(crate) fn reset_connection_state(&mut self) {
        *self = Self::default();
    }

    /// Reset the per-frame state tracking.
    /// Note: `should_rollback` and `earliest_mismatch_tick` are NOT reset here
    /// because they need to persist until consumed by `check_rollback`.
    /// A mismatch can be detected for the current/future local tick; in that
    /// case `check_rollback` defers rollback until the tick is in the local
    /// past and predicted history can be restored.
    pub(crate) fn reset_frame_state(&mut self) {
        self.received_messages_this_frame = false;
    }

    /// Reset the mismatch state after it has been consumed by check_rollback.
    pub(crate) fn reset_mismatch_state(&mut self) {
        self.earliest_mismatch_tick = None;
        self.should_rollback = false;
    }

    /// Return the pending mismatch if rollback must wait for more local
    /// prediction history.
    ///
    /// Receive-time mismatch checks only record ticks in the local past.
    /// Current-tick mismatches are recorded by the post-prediction exact
    /// confirmed check, after the prediction history for that tick exists, so
    /// they are rollback-ready on the next `check_rollback`.
    pub(crate) fn not_ready_mismatch_tick(&self, current_tick: Tick) -> Option<Tick> {
        if !self.should_rollback {
            return None;
        }
        self.earliest_mismatch_tick
            .filter(|mismatch_tick| *mismatch_tick > current_tick)
    }

    /// Consume the pending mismatch once its tick is not in the client's
    /// future and the predicted history for that tick can exist.
    pub(crate) fn take_ready_mismatch_tick(&mut self, current_tick: Tick) -> Option<Tick> {
        if !self.should_rollback {
            return None;
        }
        let mismatch_tick = self.earliest_mismatch_tick?;
        if mismatch_tick > current_tick {
            return None;
        }
        self.reset_mismatch_state();
        Some(mismatch_tick)
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
    fn mismatch_is_deferred_until_tick_is_reached() {
        let mut metadata = StateRollbackMetadata::default();
        metadata.record_mismatch(Tick(12));

        assert_eq!(metadata.not_ready_mismatch_tick(Tick(10)), Some(Tick(12)));
        assert_eq!(metadata.take_ready_mismatch_tick(Tick(10)), None);
        assert!(metadata.should_rollback);
        assert_eq!(metadata.earliest_mismatch_tick, Some(Tick(12)));

        assert_eq!(metadata.not_ready_mismatch_tick(Tick(12)), None);
        assert_eq!(metadata.take_ready_mismatch_tick(Tick(12)), Some(Tick(12)));
        assert!(!metadata.should_rollback);
        assert_eq!(metadata.earliest_mismatch_tick, None);
    }

    #[test]
    fn record_last_confirmed_tick_keeps_latest_tick() {
        let mut metadata = StateRollbackMetadata::default();

        metadata.record_last_confirmed_tick(Tick(12));
        metadata.record_last_confirmed_tick(Tick(10));
        metadata.record_last_confirmed_tick(Tick(14));

        assert_eq!(metadata.last_confirmed_tick(), Some(Tick(14)));
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

        let mut metadata = StateRollbackMetadata::default();
        metadata.record_last_confirmed_tick(Tick(900));
        assert_eq!(metadata.last_confirmed_tick(), Some(Tick(900)));
    }

    #[test]
    fn explicit_state_mismatches_keep_earliest_ready_tick() {
        let mut metadata = StateRollbackMetadata::default();

        metadata.record_mismatch(Tick(12));
        metadata.record_mismatch(Tick(14));
        metadata.record_mismatch(Tick(10));

        assert_eq!(metadata.not_ready_mismatch_tick(Tick(10)), None);
        assert_eq!(metadata.take_ready_mismatch_tick(Tick(10)), Some(Tick(10)));
    }
}

/// Store the earliest mismatched input across all remote clients.
#[derive(Debug, Reflect)]
pub struct EarliestMismatchedInput {
    /// Initialized to `Tick::MAX` so the first [`set_if_lower`] call wins.
    /// Updated via [`set_if_lower`] to track the minimum mismatch tick
    /// across all remote clients.
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
