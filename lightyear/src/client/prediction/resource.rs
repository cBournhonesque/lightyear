//! Defines bevy resources needed for Prediction

use bevy::ecs::entity::EntityHash;
use bevy::prelude::{Entity, Resource};

use crate::_reexport::ReadyBuffer;
use crate::prelude::Tick;
use crate::shared::replication::entity_map::PredictedEntityMap;

type EntityHashMap<K, V> = hashbrown::HashMap<K, V, EntityHash>;

#[derive(Resource, Default, Debug)]
pub(crate) struct PredictionManager {
    /// Map between remote and predicted entities
    pub(crate) predicted_entity_map: PredictedEntityMap,
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

impl PredictionManager {
    pub fn new() -> Self {
        Self {
            predicted_entity_map: Default::default(),
            prespawn_hash_to_entities: Default::default(),
            prespawn_tick_to_hash: Default::default(),
        }
    }
}
