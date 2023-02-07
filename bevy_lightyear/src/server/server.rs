use bevy_app::{App, Plugin};
use bevy_ecs::prelude::{Query, With, Component, Changed, Res, Entity, Added, ChangeTrackers};
use bevy_ecs::system::RemovedComponents;
use lightyear::server::Server;

pub struct ServerPlugin;


impl Plugin for ServerPlugin {
    fn build(&self, app: &mut App) {
        todo!()
    }
}


#[derive(Component)]
/// Marker component that signifies that this entity should be replicated from server to client
pub struct Replicate {
    /// If true, when this entity is despawned on the server, also despawn it on the client
    propagate_despawn: bool
}


/// Components implementing this trait will be replicated
pub trait ReplicableComponent: {}


/// System that will update entity scopes
pub fn update_scopes() {
    // despawn any entities that are not in scope anymore for a client?
}

/// Buffer any messages that need to be sent
pub fn buffer_messages() {

}

///
pub fn receive_messages() {

}

// use bevy trait queries! or use a generic system per component?
/// Query all replicable components that changed, and add them to a buffer
/// so they can be sent in the next packets sent
pub fn replicate_components<C: ReplicableComponent>(
    added_entities: Query<Entity, Added<Replicate>>,
    removed_entities: Query<RemovedComponents<Replicate>>,
    updated_components: Query<(Entity, ChangeTrackers<&C>, &C), With<Replicate>>,
    removed_components: Query<(Entity, RemovedComponents<&C>)>,
    // server: Server<>
    // scopes: Res<Scope>,
) {
    for connection in server.connections() {
        for entity in connection.scope() {
            if let Ok(entity) = added_entities.get(entity) {
                // add SpawnEntity message to the buffer of messages to send, reliably (EntityAction channel)
                server.buffer_spawn_entity(entity)

            }
            removed_entities.for_each(|entity| {
                // TODO: find a way to check the replicate component value! so we know if
                //  we despawn the entity on the client or not
                // add DespawnEntity message to the buffer of messages to send, reliably  (EntityAction channel)
            });
            updated_components.for_each(|(entity, component_tracker, component)| {
                if component_tracker.is_added() {
                    // send InsertComponent message to the buffer of messages to send, reliably  (EntityAction channel)
                } else {
                    if component_tracker.is_changed() {
                        // send UpdateComponent message to the client
                        // can be reliable or unreliable, depending on the attributes on the component
                        // ReliableEntityUpdate channel or UnreliableEntityUpdate channel
                    }
                }
            });
            removed_components.for_each(|entity| {
                // send DespawnComponent message to the client, reliably (EntityAction channel)
            });
        }
    }
}

/// Collect all things that need to get sent, and send them
pub fn send_all_packets() {
    // collect all messages to send

    // collect all replication messages to send

}