//! Defines bevy resources needed for Prediction

use alloc::vec::Vec;
use bevy_derive::{Deref, DerefMut};
use bevy_ecs::prelude::*;
use bevy_reflect::Reflect;

use crate::correction::CorrectionPolicy;
use bevy_ecs::component::HookContext;
use bevy_ecs::entity::EntityHash;
use bevy_ecs::observer::Trigger;
use bevy_ecs::query::With;
use bevy_ecs::system::Single;
use bevy_ecs::world::DeferredWorld;
use core::cell::UnsafeCell;
use core::ops::{Deref, DerefMut};
use lightyear_connection::client::Connected;
use lightyear_core::prelude::{RollbackState, SyncEvent, Tick};
use lightyear_replication::registry::buffered::BufferedChanges;
use lightyear_replication::registry::registry::ComponentRegistry;
use lightyear_replication::registry::ComponentError;
use lightyear_serde::entity_map::EntityMap;
use lightyear_sync::prelude::InputTimeline;
use lightyear_sync::timeline::input::Input;
use lightyear_utils::ready_buffer::ReadyBuffer;
use parking_lot::RwLock;
use tracing::debug;

#[derive(Resource)]
pub struct PredictionResource {
    // entity that holds the InputTimeline
    // We use this to avoid having to run a mutable query in component hook
    pub(crate) link_entity: Entity,
}

type EntityHashMap<K, V> = bevy_platform::collections::HashMap<K, V, EntityHash>;

#[derive(Default, Debug, Reflect)]
pub struct PredictedEntityMap {
    /// Map from the confirmed entity to the predicted entity
    /// useful for despawning, as we won't have access to the Confirmed/Predicted components anymore
    pub confirmed_to_predicted: EntityMap,
}

// #[derive(Debug, Clone, Copy, Default, Reflect)]
// pub enum RollbackPolicy {
//     /// Every frame, rollback to the latest of:
//     /// - last confirmed tick of any Predicted entity (all predicted entities share the same tick)
//     /// - last tick where we have a confirmed input from each remote client
//     ///
//     /// This can be useful to test that your game can handle a certain amount of rollback frames.
//     Always,
//     /// Always rollback upon receipt of a new state, without checking if the confirmed state matches
//     /// the predicted history.
//     ///
//     /// When using this policy, we don't need to store a PredictionHistory to see if the newly received
//     /// confirmed state matches the predicted state history.
//     AlwaysState,
//     #[default]
//     /// Only rollback if the confirmed state does not match the predicted state history
//     StateCheckOnly,
//     /// Rollback if the confirmed state does not match the predicted state history,
//     /// or if an input received from a remote client does not match our previous input buffer
//     StateAndInputCheck,
//     /// Only rollback be checking if the remote client inputs don't match our previously received inputs.
//     /// We will rollback to the
//     InputCheckOnly,
// }

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
    pub max_rollback_ticks: u16
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
        matches!(self.state, RollbackMode::Always) && matches!(self.input, RollbackMode::Disabled)
    }
}

/// Buffer the stores components that we need to sync from the Confirmed to the Predicted entity
#[derive(Component, Default, Deref, DerefMut, Reflect)]
pub(crate) struct PredictionSyncBuffer(BufferedChanges);

#[derive(Component, Debug, Reflect)]
#[component(on_add = PredictionManager::on_add)]
#[require(InputTimeline)]
#[require(PredictionSyncBuffer)]
pub struct PredictionManager {
    /// If true, we always rollback whenever we receive a server update, instead of checking
    /// ff the confirmed state matches the predicted state history
    pub rollback_policy: RollbackPolicy,
    /// The configuration for how to handle Correction, which is basically lerping the previously predicted state
    /// to the corrected state over a period of time after a rollback
    pub correction_policy: CorrectionPolicy,
    /// Map between confirmed and predicted entities
    ///
    /// We wrap it into an UnsafeCell because the MapEntities trait requires a mutable reference to the EntityMap,
    /// but in our case calling map_entities will not mutate the map itself; by doing so we can improve the parallelism
    /// by avoiding a `ResMut<PredictionManager>` in our systems.
    #[reflect(ignore)]
    pub predicted_entity_map: UnsafeCell<PredictedEntityMap>,
    #[doc(hidden)]
    /// Map from the hash of a PrespawnedPlayerObject to the corresponding local entity
    /// NOTE: multiple entities could share the same hash. In which case, upon receiving a server prespawned entity,
    /// we will randomly select a random entity in the set to be its predicted counterpart
    ///
    /// Also stores the tick at which the entities was spawned.
    /// If the interpolation_tick reaches that tick and there is till no match, we should despawn the entity
    pub prespawn_hash_to_entities: EntityHashMap<u64, Vec<Entity>>,
    #[doc(hidden)]
    /// Store the spawn tick of the entity, as well as the corresponding hash
    pub prespawn_tick_to_hash: ReadyBuffer<Tick, u64>,

    /// Store the most recent confirmed input across all remote clients.
    pub last_confirmed_input: LastConfirmedInput,
    /// We use a RwLock because we want to be able to update this value from multiple systems
    /// in parallel.
    #[reflect(ignore)]
    pub rollback: RwLock<RollbackState>,
    /// Rollback state from input-checks. It is computed independently of the rollback state from state-checks.
    /// Then both get merged together into `rollback`
    #[reflect(ignore)]
    pub input_rollback: RwLock<RollbackState>,
}

/// Store the most recent confirmed input across all remote clients.
///
/// This can be useful if we are using [`RollbackMode::Always`](RollbackMode), in which case we won't check
/// for state or input mismatches but simply rollback to the last tick where we had a confirmed input
/// from each remote client.
#[derive(Debug, Default, Reflect)]
pub struct LastConfirmedInput {
    pub tick: lightyear_core::tick::AtomicTick,
    pub received_any_messages: bevy_platform::sync::atomic::AtomicBool,
}

impl LastConfirmedInput {
    /// Returns true if we have received any input messages from remote clients
    pub(crate) fn received_input(&self) -> bool {
        // If we received any messages, we can assume that we have a confirmed input
        self.received_any_messages
            .load(bevy_platform::sync::atomic::Ordering::Relaxed)
    }
}

impl Default for PredictionManager {
    fn default() -> Self {
        Self {
            rollback_policy: RollbackPolicy::default(),
            correction_policy: CorrectionPolicy::default(),
            predicted_entity_map: UnsafeCell::new(PredictedEntityMap::default()),
            prespawn_hash_to_entities: EntityHashMap::default(),
            prespawn_tick_to_hash: ReadyBuffer::default(),
            last_confirmed_input: LastConfirmedInput::default(),
            rollback: RwLock::new(RollbackState::Default),
            input_rollback: RwLock::new(RollbackState::Default),
        }
    }
}

impl PredictionManager {
    fn on_add(mut deferred: DeferredWorld, context: HookContext) {
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
    /// Call MapEntities on the given component.
    ///
    /// Using this function only requires `&self` instead of `&mut self` (on the MapEntities trait), which is useful for parallelism
    pub(crate) fn map_entities<C: 'static>(
        &self,
        component: &mut C,
        component_registry: &ComponentRegistry,
    ) -> Result<(), ComponentError> {
        // SAFETY: `EntityMap` isn't mutated during `map_entities`
        unsafe {
            let entity_map = &mut *self.predicted_entity_map.get();
            component_registry.map_entities::<C>(component, &mut entity_map.confirmed_to_predicted)
        }
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

    /// Get the current input_rollback tick
    pub(crate) fn get_input_rollback_start_tick(&self) -> Option<Tick> {
        match *self.input_rollback.read().deref() {
            RollbackState::RollbackStart(start_tick) => Some(start_tick),
            RollbackState::Default => None,
        }
    }

    /// Set the rollback state back to non-rollback
    pub fn set_non_rollback(&self) {
        *self.rollback.write().deref_mut() = RollbackState::Default;
        *self.input_rollback.write().deref_mut() = RollbackState::Default;
    }

    /// Set the rollback state to `ShouldRollback` with the given tick.
    pub fn set_rollback_tick(&self, tick: Tick) {
        *self.rollback.write().deref_mut() = RollbackState::RollbackStart(tick)
    }

    /// Set the rollback state to `ShouldRollback` with the given tick.
    ///
    /// If a rollback tick was already set, overwrite only if the new rollback tick is earlier
    /// than the existing one.
    /// Returns true if a rollback tick was set, false otherwise.
    pub fn set_input_rollback_tick(&self, tick: Tick) -> bool {
        let start_tick = match *self.input_rollback.read().deref() {
            RollbackState::RollbackStart(start_tick) => Some(start_tick),
            RollbackState::Default => None,
        };
        // don't overwrite if we had an earlier rollback tick
        if start_tick.is_none_or(|start_tick| tick > start_tick) {
            debug!(rollback_tick = ?tick, "Setting rollback start");
            *self.input_rollback.write().deref_mut() = RollbackState::RollbackStart(tick);
            true
        } else {
            false
        }
    }

    pub(crate) fn handle_tick_sync(
        trigger: Trigger<SyncEvent<Input>>,
        mut manager: Single<&mut PredictionManager, With<Connected>>,
    ) {
        let data: Vec<_> = manager.prespawn_tick_to_hash.drain().collect();
        data.into_iter().for_each(|(tick, hash)| {
            manager
                .prespawn_tick_to_hash
                .push(tick + trigger.tick_delta, hash);
        });
    }
}
