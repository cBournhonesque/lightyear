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
use lightyear_core::prelude::{Tick};
use lightyear_replication::prespawn::PreSpawnedReceiver;
use lightyear_sync::prelude::InputTimelineConfig;
use parking_lot::RwLock;
use lightyear_replication::prelude::{ServerMutateTicks};

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
/// Key invariant: `ServerMutateTicks.last_tick = T` guarantees that for all entities,
/// we have complete information at tick T:
/// - Entities that received an update at T: their confirmed value is in the message
/// - Entities that didn't receive an update: their value at T = their last confirmed value
///   (because if a message was lost/in-flight, the server would resend on the next tick)
#[derive(Resource, Clone, Copy, Debug, Default, Reflect)]
pub struct StateRollbackMetadata {
    /// The last tick where we processed `ServerMutateTicks`.
    /// Used to detect when `ServerMutateTicks.last_tick` advances.
    last_processed_tick: Option<Tick>,

    /// The earliest tick where we detected a mismatch this frame.
    /// If set, we will trigger a rollback from this tick.
    pub(crate) earliest_mismatch_tick: Option<Tick>,

    /// Set to true if we received any replication message this frame.
    /// Used to trigger `RollbackMode::Always`.
    pub(crate) received_messages_this_frame: bool,

    /// Set to true if we detected a mismatch and should rollback (for RollbackMode::Check)
    pub(crate) should_rollback: bool,
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

    /// Reset the per-frame state tracking
    pub(crate) fn reset_frame_state(&mut self) {
        self.earliest_mismatch_tick = None;
        self.should_rollback = false;
        self.received_messages_this_frame = false;
    }

    /// Returns the last processed tick (for checking if ServerMutateTicks advanced)
    pub fn last_processed_tick(&self) -> Option<Tick> {
        self.last_processed_tick
    }

    /// Update the last processed tick after we've handled ServerMutateTicks advancement
    pub fn set_last_processed_tick(&mut self, tick: Tick) {
        self.last_processed_tick = Some(tick);
    }

    /// Check if ServerMutateTicks has advanced since we last processed it
    pub fn has_server_mutate_ticks_advanced(&self, server_mutate_ticks: &ServerMutateTicks) -> bool {
        let current_tick: Tick = server_mutate_ticks.last_tick().get().into();
        match self.last_processed_tick {
            None => true, // First time, always process
            Some(last) => current_tick > last,
        }
    }

    /// Get the rollback tick based on mode:
    /// - For Check mode: earliest_mismatch_tick (if any)
    /// - For Always mode: ServerMutateTicks.last_tick
    pub fn get_rollback_tick(&self, mode: RollbackMode, server_mutate_ticks: &ServerMutateTicks) -> Option<Tick> {
        match mode {
            RollbackMode::Check => {
                if self.should_rollback {
                    self.earliest_mismatch_tick
                } else {
                    None
                }
            }
            RollbackMode::Always => {
                if self.received_messages_this_frame {
                    Some(server_mutate_ticks.last_tick().get().into())
                } else {
                    None
                }
            }
            RollbackMode::Disabled => None,
        }
    }

    /// Get the tick we can safely clear histories up to.
    /// This is `ServerMutateTicks.last_tick()` since all messages for that tick were received.
    pub fn get_safe_clear_tick(server_mutate_ticks: &ServerMutateTicks) -> Tick {
        server_mutate_ticks.last_tick().get().into()
    }
}

/// Store the earliest mismatched input across all remote clients.
#[derive(Debug, Default, Reflect)]
pub struct EarliestMismatchedInput {
    pub tick: lightyear_core::tick::AtomicTick,
    pub has_mismatches: bevy_platform::sync::atomic::AtomicBool,
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
