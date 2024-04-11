//! Defines bevy resources needed for Interpolation
use bevy::ecs::reflect::ReflectResource;
use bevy::prelude::Resource;
use bevy::reflect::Reflect;

use crate::shared::replication::entity_map::InterpolatedEntityMap;

#[derive(Resource, Default, Reflect)]
#[reflect(Resource)]
pub struct InterpolationManager {
    /// Map between remote and predicted entities
    pub interpolated_entity_map: InterpolatedEntityMap,
}

impl InterpolationManager {
    pub fn new() -> Self {
        Self {
            interpolated_entity_map: Default::default(),
        }
    }
}
