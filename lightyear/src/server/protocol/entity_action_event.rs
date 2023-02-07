use bevy_ecs::component::ComponentId;
use bevy_ecs::entity::Entity;

#[derive(Clone, PartialEq, Eq)]
pub enum EntityActionEvent {
    SpawnEntity(Entity),
    DespawnEntity(Entity),
    InsertComponent(Entity, ComponentId),
    RemoveComponent(Entity, ComponentId),
}
