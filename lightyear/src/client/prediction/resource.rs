//! Defines bevy resources needed for Prediction

use bevy::prelude::Resource;

use crate::shared::replication::entity_map::PredictedEntityMap;

#[derive(Resource, Default)]
pub struct PredictionManager {
    /// Map between remote and predicted entities
    pub predicted_entity_map: PredictedEntityMap,
}

impl PredictionManager {
    pub fn new() -> Self {
        Self {
            predicted_entity_map: Default::default(),
        }
    }
}
