use std::collections::HashSet;
use bevy_ecs::entity::Entity;
use crate::server::RoomKey;

use crate::shared::ComponentId;


pub struct GlobalEntityRecord {
    pub room_key: Option<RoomKey>,
    pub entity_handle: Entity,
    pub component_kinds: HashSet<ComponentId>,
}

impl GlobalEntityRecord {
    pub fn new(entity_handle: Entity) -> Self {
        Self {
            room_key: None,
            entity_handle,
            component_kinds: HashSet::new(),
        }
    }
}
