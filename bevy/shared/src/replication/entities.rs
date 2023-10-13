// systems to replicate entities

// use reliable channel for entity actions (spawn/despawn entity)
// for component insert/remove, let user choose if we use reliable or unreliable channel

// entity_created
// entity_removed
// component inserted
// component removed

use bevy_ecs::entity::Entity;

// enum EntityAction<C: ComponentProtocol> {

pub enum ReplicationMessage<C> {
    SpawnEntity(Entity),
    DespawnEntity(Entity),
    InsertComponent(Entity, C),
    RemoveComponent(Entity), // TODO: add type of component
    EntityUpdate(Entity, Vec<C>),
}

// use unreliable channel for component updates?

// component updates: iterate on all components in the archetype, similar to bevy_replicon

// 1. server-spawned entities are sent to the client via reliable-channel
// 2. wait for ack to be sure that entity has been spawned on the client
