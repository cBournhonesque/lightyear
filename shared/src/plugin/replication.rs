use bevy::ecs::component::ComponentId;
use bevy::prelude::{Entity, FromWorld, Resource, World};
use std::collections::HashMap;

use crate::replication::Replicate;

#[derive(Resource)]
pub struct ReplicationData {
    /// ComponentId of the Replicate component
    pub replication_id: ComponentId,
    // TODO: maybe add a map from Component to the corresponding systems
    /// Map of the replicated entities that are owned by the current world
    pub owned_entities: HashMap<Entity, Replicate>,
    // pub received_entities: HashMap<Entity, Replicate>,
}

impl FromWorld for ReplicationData {
    fn from_world(world: &mut World) -> Self {
        Self {
            replication_id: world.init_component::<Replicate>(),
            owned_entities: HashMap::new(),
            // received_entities: HashMap::new(),
        }
    }
}

impl ReplicationData {
    /// Returns true if the component is in the ComponentProtocol
    pub fn contains_component(&self, component_id: ComponentId) -> bool {
        todo!()
    }
}
