//! Defines bevy resources needed for Prediction

use bevy_ecs::prelude::*;
use bevy_reflect::Reflect;

use crate::correction::CorrectionPolicy;
use crate::rollback::RollbackState;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use bevy_ecs::entity::EntityHashSet;
use core::ops::{Deref, DerefMut};
use lightyear_core::prelude::Tick;
use lightyear_sync::prelude::InputTimelineConfig;
use parking_lot::RwLock;

/// Prediction checks deferred because an authoritative update was ahead of the local timeline.
///
/// Receive-time rollback checks cannot compare an update after the current local tick with
/// prediction history yet. Replicon's `ConfirmHistory` still records that the entity was updated,
/// however, so the prediction scan's usual `ConfirmHistory` optimization would later skip the
/// entity entirely. This index remembers those entities by authoritative tick until the
/// completed server frontier makes them checkable.
///
/// [`PredictionRegistry::check_rollback_for_unchanged_component`](crate::registry::PredictionRegistry::check_rollback_for_unchanged_component)
/// handles each drained entity at that frontier: it uses an explicit confirmed component sample
/// when one exists, and records the component as unchanged only when no sample exists at the tick.
#[doc(hidden)]
#[derive(Debug, Default)]
pub struct PendingEntityStateChecks {
    by_tick: BTreeMap<Tick, EntityHashSet>,
}

impl PendingEntityStateChecks {
    pub(crate) fn record(&mut self, tick: Tick, entity: Entity) {
        self.by_tick.entry(tick).or_default().insert(entity);
    }

    /// Removes and returns all entities whose deferred update is now checkable.
    pub(crate) fn take_through(&mut self, completed_tick: Tick) -> EntityHashSet {
        let mut entities = EntityHashSet::default();
        while self
            .by_tick
            .first_key_value()
            .is_some_and(|(tick, _)| *tick <= completed_tick)
        {
            let (_, pending) = self.by_tick.pop_first().unwrap();
            entities.extend(pending);
        }
        entities
    }

    pub(crate) fn clear(&mut self) {
        self.by_tick.clear();
    }

    /// Returns whether an entity has a deferred state check at `tick`.
    #[cfg(any(test, feature = "test_utils"))]
    pub fn contains(&self, tick: Tick, entity: Entity) -> bool {
        self.by_tick
            .get(&tick)
            .is_some_and(|entities| entities.contains(&entity))
    }
}

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
    /// Upper bound on the number of ticks we can roll back.
    ///
    /// A non-zero [`InputTimelineConfig::maximum_predicted_ticks`] can lower the effective bound.
    /// Rollback requests beyond the effective bound are ignored.
    pub max_rollback_ticks: u16,
}

impl Default for RollbackPolicy {
    fn default() -> Self {
        Self {
            state: RollbackMode::Check,
            input: RollbackMode::Check,
            max_rollback_ticks: 20,
        }
    }
}

impl RollbackPolicy {
    /// Maximum rollback depth after applying the input timing configuration's prediction limit.
    ///
    /// A non-zero `maximum_predicted_ticks` bounds how far ahead the local simulation can be, so
    /// rollback state older than that cannot be needed. Zero denotes lockstep and does not cap
    /// explicit or forced state rollbacks.
    pub fn effective_max_rollback_ticks(&self, input_config: &InputTimelineConfig) -> u16 {
        let maximum_predicted_ticks = input_config.maximum_predicted_ticks();
        if maximum_predicted_ticks == 0 {
            self.max_rollback_ticks
        } else {
            self.max_rollback_ticks.min(maximum_predicted_ticks)
        }
    }

    /// Returns true if we don't need to store a prediction history.
    ///
    /// PredictionHistory is not needed if we always rollback on new states
    pub fn no_prediction_history(&self) -> bool {
        !matches!(self.state, RollbackMode::Disabled)
            && matches!(self.input, RollbackMode::Disabled)
    }
}

/// Application-global state that enables prediction and rollback.
///
/// [`PredictionPlugin`](crate::prelude::PredictionPlugin) installs the prediction systems, but
/// those systems only run when this resource is present and the cached network topology is a
/// conventional client or P2P session. Insert one manager into the application when it should
/// create predicted entities, record prediction history, and perform one global
/// rollback/reconciliation pipeline.
#[derive(Resource, Debug, Reflect)]
pub struct PredictionManager {
    /// Configuration for how rollbacks are triggered
    pub rollback_policy: RollbackPolicy,
    /// Configuration for smoothing the rollback error over time
    pub correction_policy: CorrectionPolicy,
    /// For input-based rollback: tracks earliest mismatch across remote clients
    pub earliest_mismatch_input: EarliestMismatchedInput,

    /// Earliest tick that an input rollback may restore.
    ///
    /// A deterministic P2P session sets this to the tick immediately before its agreed first
    /// gameplay tick. That boundary is the session's initial world snapshot.
    #[doc(hidden)]
    pub input_rollback_floor: Option<Tick>,

    #[doc(hidden)]
    pub deterministic_despawn: Vec<(Tick, Entity)>,
    #[doc(hidden)]
    pub deterministic_skip_despawn: Vec<(Tick, Entity)>,
    /// Receive-time state checks deferred until the authoritative tick is locally checkable.
    ///
    /// See [`PendingEntityStateChecks`] for why the completed-tick rollback scan needs this index.
    #[doc(hidden)]
    #[reflect(ignore)]
    pub pending_entity_state_checks: PendingEntityStateChecks,
    #[doc(hidden)]
    #[reflect(ignore)]
    pub rollback: RwLock<RollbackState>,
}

/// Application-global frontier of confirmed input across all remote players.
///
/// This is a resource because the rollback decision combines every remote input stream in the
/// application; it does not belong to any one network link.
#[derive(Resource, Debug, Reflect)]
pub struct LastConfirmedInput {
    /// Current frame's aggregate, updated via [`AtomicTick::set_if_lower`] to track the minimum
    /// last-confirmed tick across all remote clients. Reset to a high value each frame by
    /// [`reset_input_rollback_tracker`] so the minimum is computed correctly.
    ///
    /// [`AtomicTick::set_if_lower`]: lightyear_core::tick::AtomicTick::set_if_lower
    /// [`reset_input_rollback_tracker`]: crate::rollback::reset_input_rollback_tracker
    pub tick: lightyear_core::tick::AtomicTick,
    /// Completed aggregate from the previous frame.
    ///
    /// Rollback uses this explicitly so inputs received in the current frame are replayed from the
    /// frontier that was known before those inputs arrived.
    pub previous_frame_tick: Tick,
    pub received_any_messages: bevy_platform::sync::atomic::AtomicBool,
    /// Set to true if the current aggregate contains inputs from all remote clients.
    pub received_for_all_clients: bool,
}

impl Default for LastConfirmedInput {
    fn default() -> Self {
        Self {
            tick: lightyear_core::tick::AtomicTick::new_max(),
            previous_frame_tick: Tick(u32::MAX),
            received_any_messages: bevy_platform::sync::atomic::AtomicBool::new(false),
            received_for_all_clients: false,
        }
    }
}

impl LastConfirmedInput {
    /// Returns true if we've received any confirmed input from remote clients this frame
    pub fn received_input(&self) -> bool {
        self.received_any_messages
            .load(bevy_platform::sync::atomic::Ordering::Relaxed)
    }

    /// Return the last confirmed input tick, or None if we haven't received any confirmed input yet.
    pub fn get(&self) -> Option<Tick> {
        match self.tick.get() {
            tick if tick == Tick(u32::MAX) => None,
            tick => Some(tick),
        }
    }

    /// Return the confirmed-input frontier completed during the previous frame.
    pub fn previous_frame(&self) -> Option<Tick> {
        match self.previous_frame_tick {
            tick if tick == Tick(u32::MAX) => None,
            tick => Some(tick),
        }
    }

    /// Preserve the current aggregate for systems that must consume the previous frame's view.
    pub fn finalize_frame(&mut self) {
        self.previous_frame_tick = self.tick.get();
    }
}

/// Stores metadata related to state-based prediction.
#[derive(Resource, Clone, Copy, Debug, Default, Reflect)]
pub struct StateRollbackMetadata {
    /// Latest completed server mutate tick consumed by the rollback check.
    ///
    /// Replicon advances `ServerMutateTicks` during receive; this advances only after
    /// `check_rollback` handles that frontier. We retain it separately to:
    /// - avoid rescanning unchanged entities while the completed frontier is unchanged;
    /// - let receive functions ignore late updates older than an already-checked frontier;
    /// - provide a safe watermark for pruning diff history.
    ///
    /// It can lag behind `ServerMutateTicks` when a completed tick is ahead of the local timeline.
    last_processed_tick: Option<Tick>,

    /// Earliest receive-time state mismatch not yet covered by a completed rollback check.
    ///
    /// A mismatch can be detected before Replicon has completed all mutate messages through that
    /// tick. We retain the earliest such tick until the completed server frontier reaches it, then
    /// roll back from that latest globally complete frontier. Later confirmed samples do not need
    /// separate mismatch entries: they remain in `ConfirmedHistory` and are applied during replay.
    earliest_pending_mismatch_tick: Option<Tick>,

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
    /// Record a receive-time mismatch at `tick`.
    ///
    /// Only the earliest pending mismatch matters. Once the globally completed server frontier
    /// reaches it, rollback starts from that completed frontier and replay applies every later
    /// confirmed sample retained in `ConfirmedHistory`.
    ///
    /// Returns `true` because the unbounded representation can retain every mismatch tick. The
    /// return value is kept for compatibility with the previous bounded representation, which
    /// returned `false` when a tick fell outside its 64-tick window.
    pub fn record_mismatch(&mut self, tick: Tick) -> bool {
        match self.earliest_pending_mismatch_tick {
            None => self.earliest_pending_mismatch_tick = Some(tick),
            Some(existing) if tick < existing => self.earliest_pending_mismatch_tick = Some(tick),
            _ => {}
        }
        true
    }

    /// Return the earliest pending mismatch once `completed_tick` has reached it.
    pub(crate) fn pending_mismatch_at_or_before(&self, completed_tick: Tick) -> Option<Tick> {
        self.earliest_pending_mismatch_tick
            .filter(|mismatch_tick| *mismatch_tick <= completed_tick)
    }

    /// Return whether receive-time prediction checks should run for `tick`.
    pub(crate) fn should_check_mismatch_at(&self, tick: Tick) -> bool {
        if self
            .last_processed_tick
            .is_some_and(|last_processed| tick < last_processed)
        {
            return false;
        }
        // A later mismatch cannot make rollback eligible any sooner. Keep checking earlier ticks,
        // though, because an out-of-order confirmed update can move the pending frontier earlier.
        self.earliest_pending_mismatch_tick
            .is_none_or(|pending_tick| tick < pending_tick)
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
    /// Note: the pending mismatch is NOT reset here because receive-time mismatch
    /// evidence persists until the completed server tick is processed.
    pub(crate) fn reset_frame_state(&mut self) {
        self.received_messages_this_frame = false;
    }

    /// Clear all retained mismatch evidence.
    pub fn clear_mismatch_history(&mut self) {
        self.earliest_pending_mismatch_tick = None;
    }

    /// Returns the latest completed server mutate tick consumed by rollback checking.
    ///
    /// During Replicon's receive systems this still reflects the previous rollback-check frontier;
    /// it is advanced only after the current completed tick has been handled by `check_rollback`.
    /// It is used both to avoid rescanning an already-consumed completed tick and to reject stale
    /// receive-time mismatch checks for ticks older than that frontier.
    pub fn last_processed_tick(&self) -> Option<Tick> {
        self.last_processed_tick
    }

    /// Record that rollback checking consumed this completed server mutate tick.
    ///
    /// Call this only at the end of the rollback check, after either consuming an explicit mismatch
    /// or scanning unchanged entities. Do not advance it directly from `ServerMutateTicks`, or while
    /// the completed tick is still in the client's future: receive functions must continue to see
    /// the previously processed frontier while the current frame's replication is being applied.
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
    use lightyear_sync::timeline::input::InputDelayConfig;

    #[test]
    fn effective_max_rollback_ticks_respects_prediction_limit() {
        let mut policy = RollbackPolicy::default();
        assert_eq!(policy.max_rollback_ticks, 20);

        let balanced =
            InputTimelineConfig::default().with_input_delay(InputDelayConfig::balanced());
        assert_eq!(policy.effective_max_rollback_ticks(&balanced), 7);

        policy.max_rollback_ticks = 5;
        assert_eq!(policy.effective_max_rollback_ticks(&balanced), 5);

        let lockstep =
            InputTimelineConfig::default().with_input_delay(InputDelayConfig::no_prediction());
        assert_eq!(policy.effective_max_rollback_ticks(&lockstep), 5);
    }

    #[test]
    fn last_confirmed_input_default_starts_unset() {
        let mut last_confirmed_input = LastConfirmedInput::default();

        assert_eq!(last_confirmed_input.tick.get(), Tick(u32::MAX));
        assert_eq!(last_confirmed_input.get(), None);
        assert_eq!(last_confirmed_input.previous_frame(), None);
        assert!(!last_confirmed_input.received_input());

        last_confirmed_input.tick.set_if_lower(Tick(12));
        last_confirmed_input.received_for_all_clients = true;
        last_confirmed_input.finalize_frame();
        assert_eq!(last_confirmed_input.previous_frame(), Some(Tick(12)));
    }

    #[test]
    fn pending_mismatch_keeps_earliest_tick() {
        let mut metadata = StateRollbackMetadata::default();
        metadata.set_last_processed_tick(Tick(10));
        metadata.record_mismatch(Tick(12));

        assert_eq!(metadata.pending_mismatch_at_or_before(Tick(11)), None);
        assert_eq!(
            metadata.pending_mismatch_at_or_before(Tick(12)),
            Some(Tick(12))
        );
        assert!(!metadata.should_check_mismatch_at(Tick(9)));
        assert!(!metadata.should_check_mismatch_at(Tick(12)));
        assert!(!metadata.should_check_mismatch_at(Tick(13)));
        assert!(metadata.should_check_mismatch_at(Tick(11)));

        metadata.record_mismatch(Tick(14));
        assert_eq!(metadata.earliest_pending_mismatch_tick, Some(Tick(12)));

        metadata.record_mismatch(Tick(11));
        assert_eq!(metadata.earliest_pending_mismatch_tick, Some(Tick(11)));

        metadata.clear_mismatch_history();
        assert_eq!(metadata.earliest_pending_mismatch_tick, None);
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
    fn completed_frontier_can_jump_past_pending_mismatch() {
        let mut metadata = StateRollbackMetadata::default();
        metadata.set_last_processed_tick(Tick(100));
        metadata.record_mismatch(Tick(105));

        assert_eq!(metadata.pending_mismatch_at_or_before(Tick(104)), None);
        assert_eq!(
            metadata.pending_mismatch_at_or_before(Tick(106)),
            Some(Tick(105))
        );
    }

    #[test]
    fn pending_mismatch_has_no_fixed_tick_window() {
        let mut metadata = StateRollbackMetadata::default();
        metadata.set_last_processed_tick(Tick(10));
        metadata.record_mismatch(Tick(100));

        assert_eq!(
            metadata.pending_mismatch_at_or_before(Tick(100)),
            Some(Tick(100))
        );
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
            input_rollback_floor: None,
            deterministic_skip_despawn: Vec::default(),
            deterministic_despawn: Vec::default(),
            pending_entity_state_checks: PendingEntityStateChecks::default(),
            rollback: RwLock::new(RollbackState::Default),
        }
    }
}

// SAFETY: We never use UnsafeCell to mutate the predicted_entity_map, so it's safe to send and sync
unsafe impl Send for PredictionManager {}
unsafe impl Sync for PredictionManager {}

impl PredictionManager {
    /// Returns whether restoring `rollback_tick` stays within the active session's input history.
    pub(crate) fn input_rollback_is_allowed(&self, rollback_tick: Tick) -> bool {
        self.input_rollback_floor
            .is_none_or(|floor| rollback_tick >= floor)
    }

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
