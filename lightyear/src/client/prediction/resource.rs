//! Defines bevy resources needed for Prediction

#[cfg(not(feature = "std"))]
use alloc::vec::Vec;
use bevy::ecs::entity::EntityHash;
use bevy::prelude::{Entity, Resource};
use core::cell::UnsafeCell;

use crate::prelude::{ComponentRegistry, Tick};
use crate::protocol::component::ComponentError;
use crate::shared::replication::entity_map::PredictedEntityMap;
use crate::utils::ready_buffer::ReadyBuffer;

type EntityHashMap<K, V> = bevy::platform::collections::HashMap<K, V, EntityHash>;

#[derive(Resource, Default, Debug)]
pub(crate) struct PredictionManager {
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
}

// SAFETY: We never use UnsafeCell to mutate the predicted_entity_map, so it's safe to send and sync
unsafe impl Send for PredictionManager {}
unsafe impl Sync for PredictionManager {}

impl PredictionManager {
    pub(crate) fn new() -> Self {
        Self {
            predicted_entity_map: Default::default(),
            prespawn_hash_to_entities: Default::default(),
            prespawn_tick_to_hash: Default::default(),
        }
    }

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
