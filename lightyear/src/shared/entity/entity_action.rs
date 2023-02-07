use bevy_ecs::component::ComponentId;
use bevy_ecs::entity::Entity;


pub enum EntityAction {
    SpawnEntity(Entity, Vec<ComponentId>),
    DespawnEntity(Entity),
    InsertComponent(Entity, ComponentId),
    RemoveComponent(Entity, ComponentId),
    Noop,
}

impl EntityAction {
    pub fn entity(&self) -> Option<Entity> {
        match self {
            EntityAction::SpawnEntity(entity, _) => Some(*entity),
            EntityAction::DespawnEntity(entity) => Some(*entity),
            EntityAction::InsertComponent(entity, _) => Some(*entity),
            EntityAction::RemoveComponent(entity, _) => Some(*entity),
            EntityAction::Noop => None,
        }
    }
}
