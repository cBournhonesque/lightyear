//! Defines bevy resources needed for Prediction

#[cfg(not(feature = "std"))]
use alloc::vec::Vec;

use bevy::ecs::component::HookContext;
use bevy::ecs::entity::EntityHash;
use bevy::ecs::world::DeferredWorld;
use bevy::prelude::{Component, Entity, Reflect, Resource, World};
use core::cell::UnsafeCell;
use lightyear_core::prelude::Tick;
use lightyear_replication::receive::TempWriteBuffer;
use lightyear_replication::registry::registry::ComponentRegistry;
use lightyear_replication::registry::ComponentError;
use lightyear_serde::entity_map::EntityMap;
use lightyear_utils::ready_buffer::ReadyBuffer;

#[derive(Resource)]
pub struct PredictionResource {
    // entity that holds the InputTimeline
    // We use this to avoid having to run a mutable query in component hook
    pub(crate) link_entity: Entity,
}

type EntityHashMap<K, V> = bevy::platform::collections::HashMap<K, V, EntityHash>;

#[derive(Default, Debug, Reflect)]
pub struct PredictedEntityMap {
    /// Map from the confirmed entity to the predicted entity
    /// useful for despawning, as we won't have access to the Confirmed/Predicted components anymore
    pub confirmed_to_predicted: EntityMap,
}

#[derive(Resource, Component, Debug)]
#[component(on_add = PredictionManager::on_add)]
pub struct PredictionManager {
    /// If true, we always rollback whenever we receive a server update, instead of checking
    /// ff the confirmed state matches the predicted state history
    pub always_rollback: bool,
    /// The number of correction ticks will be a multiplier of the number of ticks between
    /// the client and the server correction
    /// (i.e. if the client is 10 ticks head and correction_ticks is 1.0, then the correction will be done over 10 ticks)
    // Number of ticks it will take to visually update the Predicted state to the new Corrected state
    pub correction_ticks_factor: f32,
    /// Map between confirmed and predicted entities
    ///
    /// We wrap it into an UnsafeCell because the MapEntities trait requires a mutable reference to the EntityMap,
    /// but in our case calling map_entities will not mutate the map itself; by doing so we can improve the parallelism
    /// by avoiding a `ResMut<PredictionManager>` in our systems.
    pub(crate) predicted_entity_map: UnsafeCell<PredictedEntityMap>,
    /// Map from the hash of a PrespawnedPlayerObject to the corresponding local entity
    /// NOTE: multiple entities could share the same hash. In which case, upon receiving a server prespawned entity,
    /// we will randomly select a random entity in the set to be its predicted counterpart
    ///
    /// Also stores the tick at which the entities was spawned.
    /// If the interpolation_tick reaches that tick and there is till no match, we should despawn the entity
    pub(crate) prespawn_hash_to_entities: EntityHashMap<u64, Vec<Entity>>,
    /// Store the spawn tick of the entity, as well as the corresponding hash
    pub(crate) prespawn_tick_to_hash: ReadyBuffer<Tick, u64>,
    pub(crate) temp_write_buffer: TempWriteBuffer,
}

impl Default for PredictionManager {
    fn default() -> Self {
        Self {
            always_rollback: false,
            correction_ticks_factor: 1.0,
            predicted_entity_map: UnsafeCell::new(PredictedEntityMap::default()),
            prespawn_hash_to_entities: EntityHashMap::default(),
            prespawn_tick_to_hash: ReadyBuffer::default(),
            temp_write_buffer: TempWriteBuffer::default(),
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
}
