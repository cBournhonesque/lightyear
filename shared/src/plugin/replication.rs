use bevy::ecs::component::ComponentId;
use bevy::prelude::{FromWorld, Resource, World};

use crate::replication::Replicate;

#[derive(Resource)]
pub struct ReplicationData {
    /// ComponentId of the Replicate component
    pub replication_id: ComponentId,
    // TODO: maybe add a map from Component to the corresponding systems
}

impl FromWorld for ReplicationData {
    fn from_world(world: &mut World) -> Self {
        Self {
            replication_id: world.init_component::<Replicate>(),
        }
    }
}

impl ReplicationData {
    /// Returns true if the component is in the ComponentProtocol
    pub fn contains_component(&self, component_id: ComponentId) -> bool {
        todo!()
    }
}
