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
use bevy_derive::{Deref, DerefMut};
use lightyear_core::prelude::{Predicted, Tick};
use lightyear_replication::prespawn::PreSpawnedReceiver;
use lightyear_sync::prelude::InputTimelineConfig;
use parking_lot::RwLock;
use seahash::State;
use lightyear_replication::prelude::{ConfirmHistory, ServerMutateTicks};

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

// TODO: there is ambiguity on how to handle AlwaysState and AlwaysInput.
//  Let's say we have T1 = last confirmed tick, T2 = last confirmed input tick.
//  We could either:
//  - store a PredictionHistory and rollback to the earliest to T1/T2
//  - not store a prediction history, and if T2 > T1, restore to T1. (T1 > T2 should not happen)
// I guess the simplest to just store prediction history for now.
impl RollbackPolicy {
    // TODO: use this to maybe not store prediction history at all!
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
// TODO: ideally we would only insert LastConfirmedInput if the PredictionManager is updated to use RollbackMode::AlwaysInput
//  because that's where we need it. In practice we don't have an OnChange observer so we cannot do this easily
#[require(LastConfirmedInput)]
// TODO: ideally we would only insert LastConfirmedTick only if state rollback is enabled
pub struct PredictionManager {
    /// If true, we always rollback whenever we receive a server update, instead of checking
    /// ff the confirmed state matches the predicted state history
    pub rollback_policy: RollbackPolicy,
    /// The configuration for how to handle Correction, which is basically lerping the previously predicted state
    /// to the corrected state over a period of time after a rollback
    pub correction_policy: CorrectionPolicy,
    pub earliest_mismatch_input: EarliestMismatchedInput,

    // NOTE: this is pub because ..Default::default() syntax needs all fields to be pub
    // For deterministic entity that might be despawned if there is a rollback,
    // record their tick here so we can iterate through them.
    // The Vec is ordered by Tick.
    #[doc(hidden)]
    pub deterministic_despawn: Vec<(Tick, Entity)>,
    // For deterministic skip despawn, at the time of rollback we iterate through them
    // and either insert DisableRollback if has not been long, or remove it
    #[doc(hidden)]
    pub deterministic_skip_despawn: Vec<(Tick, Entity)>,
    // // TODO: this needs to be cleaned up at regular intervals!
    // //  do a centralized TickCleanup system in lightyear_core
    // /// The tick when we last did a rollback. This is used to prevent rolling back multiple times to the same tick.
    // pub last_rollback_tick: Option<Tick>,
    /// We use a RwLock because we want to be able to update this value from multiple systems
    /// in parallel.
    #[doc(hidden)]
    #[reflect(ignore)]
    pub rollback: RwLock<RollbackState>,
}

/// Store the most recent confirmed input across all remote clients.
///
/// This can be useful if we are using [`RollbackMode::Always`](RollbackMode), in which case we won't check
/// for state or input mismatches but simply rollback to the last tick where we had a confirmed input
/// from each remote client.
#[derive(Component, Debug, Default, Reflect)]
pub struct LastConfirmedInput {
    pub tick: lightyear_core::tick::AtomicTick,
    pub received_any_messages: bevy_platform::sync::atomic::AtomicBool,
}

impl LastConfirmedInput {
    /// Returns true if we have received any input messages from remote clients
    pub fn received_input(&self) -> bool {
        // If we received any messages, we can assume that we have a confirmed input
        self.received_any_messages
            .load(bevy_platform::sync::atomic::Ordering::Relaxed)
    }
}


/// Stores metadata related to state-based prediction.
#[derive(Resource, Clone, Copy, Debug, Default, Reflect)]
pub struct StateRollbackMetadata {
    /// Stores the earliest confirmed tick across all [`Predicted`] entities.
    ///
    /// Whenever we receive a replication message for a predicted entity, we potentially rollback from this tick.
    ///
    /// There is some subtlety in how this is computed:
    /// - [`ServerMutateTicks`] contains the information of whether a tick was confirmed across ALL replicated entities. In general it is possible
    ///   that some entities have a more recent confirmed tick compared to [`ServerMutateTicks`] (if we didn't receive all mutate messages for a given tick)
    /// - We also send an empty mutation message if there were no changes, to avoid leaving the client in a state where they mispredict an entity
    ///   and there is no replication update to force a rollback. In these cases the entity's [`ConfirmHistory`] is not updated but the
    ///   [`ServerMutateTicks`] is
    last_confirmed_tick: Tick,
    // Will be set to true if we received any replication message this frame
    pub(crate) received_messages_this_frame: bool,
    pub(crate) should_rollback: bool,
}

// TODO: THESE ARE REPLICON TICKS CORRESPONDING TO THE SENDER'S REPLICATION INTERVAL!!
//  TOTALLY UNRELATED TO THE RECEIVER'S TICK! OR MAYBE THEY ARE? SINCE THE REPLICON TICK IS
//  INCREMENTED BY LIGHTYEAR's TICK?

impl StateRollbackMetadata {
    pub fn last_confirmed_tick(&self) -> Tick {
        self.last_confirmed_tick
    }

    pub(crate) fn update_last_confirmed_tick(
        mut metadata: ResMut<StateRollbackMetadata>,
        server_mutate_ticks: Res<ServerMutateTicks>,
        confirm_history: Query<&ConfirmHistory, With<Predicted>>,
    ) {
        let current = metadata.last_confirmed_tick;
        // - at minimum it's `ServerMutateTicks`.last_tick()
        let base: Tick = server_mutate_ticks.last_tick().get().into();
        // - if predicted entities are a subset of all replicated entities, it's the minimum of ConfirmHistory across all predicted entities
        let mut minimum = base + 1000;
        for history in confirm_history.iter() {
            minimum = core::cmp::min(minimum, history.last_tick().get().into());
        }
        let new = core::cmp::max(base, minimum);
        if current != new {
            metadata.last_confirmed_tick = new;
        }
    }
}

/// Store the earliest mismatched input across all remote clients.
///
/// This is if we are using [`RollbackMode::Check`](RollbackMode), in which case we
/// we will start the rollback from the earliest mismatched input tick across the
/// inputs from all remote clients.
#[derive(Debug, Default, Reflect)]
pub struct EarliestMismatchedInput {
    pub tick: lightyear_core::tick::AtomicTick,
    pub has_mismatches: bevy_platform::sync::atomic::AtomicBool,
}

impl EarliestMismatchedInput {
    /// Returns true if we have any input mismatch
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
            // last_rollback_tick: None,
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
