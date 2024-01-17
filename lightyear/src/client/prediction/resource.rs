//! Defines bevy resources needed for Prediction

use bevy::prelude::{Entity, Resource};
use bevy::utils::EntityHashMap;

use crate::shared::replication::entity_map::PredictedEntityMap;

#[derive(Resource, Default)]
pub struct PredictionManager {
    /// Map between remote and predicted entities
    pub predicted_entity_map: PredictedEntityMap,
    /// Map from the hash of a PrespawnedPlayerObject to the corresponding local entity
    pub prespawn_entities_map: EntityHashMap<u64, Entity>,
}

impl PredictionManager {
    pub fn new() -> Self {
        Self {
            predicted_entity_map: Default::default(),
            prespawn_entities_map: Default::default(),
        }
    }
}
