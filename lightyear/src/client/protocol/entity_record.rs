use std::collections::HashSet;
use bevy_ecs::entity::Entity;

use crate::shared::{ComponentId, NetEntity};

pub struct EntityRecord {
    pub net_entity: NetEntity,
    pub component_kinds: HashSet<ComponentId>,
    pub entity_handle: Entity,
}

impl EntityRecord {
    pub fn new(net_entity: NetEntity, entity_handle: Entity) -> Self {
        EntityRecord {
            net_entity,
            component_kinds: HashSet::new(),
            entity_handle,
        }
    }
}
