//! Defines the server bevy systems and run conditions
use bevy::prelude::{Events, Mut, Res, ResMut, Time, World};
use tracing::{debug, error, trace};

use crate::_reexport::ComponentProtocol;
use crate::connection::events::{IterEntityDespawnEvent, IterEntitySpawnEvent};
use crate::protocol::message::MessageProtocol;
use crate::protocol::Protocol;
use crate::server::events::{ConnectEvent, DisconnectEvent, EntityDespawnEvent, EntitySpawnEvent};
use crate::server::resource::Server;
use crate::shared::replication::ReplicationSend;

pub(crate) fn receive<P: Protocol>(world: &mut World) {
    trace!("Receive client packets");
    world.resource_scope(|world, mut server: Mut<Server<P>>| {
        let time = world.get_resource::<Time>().unwrap();

        // update client state, send keep-alives, receive packets from io
        server.update(time.delta()).unwrap();
        // buffer packets into message managers
        server.recv_packets().unwrap();

        // receive events
        server.receive(world);

        // Write the received events into bevy events
        if !server.events().is_empty() {
            // TODO: write these as systems? might be easier to also add the events to the app
            //  it might just be less efficient? + maybe tricky to
            // Input events
            // Update the input buffers with any InputMessage received:

            // ADD A FUNCTION THAT ITERATES THROUGH EACH CONNECTION AND RETURNS InputEvent for THE CURRENT TICK

            // Connection / Disconnection events
            if server.events().has_connections() {
                let mut connect_event_writer =
                    world.get_resource_mut::<Events<ConnectEvent>>().unwrap();
                for client_id in server.events().iter_connections() {
                    debug!("Client connected event: {}", client_id);
                    connect_event_writer.send(ConnectEvent::new(client_id));
                }
            }

            if server.events().has_disconnections() {
                let mut connect_event_writer =
                    world.get_resource_mut::<Events<DisconnectEvent>>().unwrap();
                for client_id in server.events().iter_disconnections() {
                    debug!("Client disconnected event: {}", client_id);
                    connect_event_writer.send(DisconnectEvent::new(client_id));
                }
            }

            // Message Events
            P::Message::push_message_events(world, server.events());

            // EntitySpawn Events
            if server.events().has_entity_spawn() {
                let mut entity_spawn_event_writer = world
                    .get_resource_mut::<Events<EntitySpawnEvent>>()
                    .unwrap();
                for (entity, client_id) in server.events().into_iter_entity_spawn() {
                    entity_spawn_event_writer.send(EntitySpawnEvent::new(entity, client_id));
                }
            }
            // EntityDespawn Events
            if server.events().has_entity_despawn() {
                let mut entity_despawn_event_writer = world
                    .get_resource_mut::<Events<EntityDespawnEvent>>()
                    .unwrap();
                for (entity, client_id) in server.events().into_iter_entity_spawn() {
                    entity_despawn_event_writer.send(EntityDespawnEvent::new(entity, client_id));
                }
            }

            // Update component events (updates, inserts, removes)
            P::Components::push_component_events(world, server.events());
        }
    });
}

// or do additional send stuff here
pub(crate) fn send<P: Protocol>(world: &mut World) {
    trace!("Send packets to clients");
    world.resource_scope(|world, mut server: Mut<Server<P>>| {
        // finalize any packets that are needed for replication
        server
            .buffer_replication_messages(world.change_tick())
            .unwrap_or_else(|e| {
                error!("Error preparing replicate send: {}", e);
            });
        // send buffered packets to io
        server.send_packets().unwrap();

        // clear the list of newly connected clients
        // (cannot just use the ConnectionEvent because it is cleared after each frame)
        server.connection_manager.new_clients.clear();

        // TODO: clear the dependency graph for replication groups send
    });
}

/// Clear the received events
/// We put this in a separate as send because we want to run this every frame, and
/// Send only runs every send_interval
pub(crate) fn clear_events<P: Protocol>(mut server: ResMut<Server<P>>) {
    server.clear_events();
}

pub(crate) fn is_ready_to_send<P: Protocol>(server: Res<Server<P>>) -> bool {
    server.is_ready_to_send()
}
