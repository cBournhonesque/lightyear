//! Defines the client bevy systems and run conditions
use bevy::prelude::{Events, Fixed, Mut, Res, ResMut, Time, Virtual, World};
use tracing::trace;

use crate::client::events::{EntityDespawnEvent, EntitySpawnEvent};
use crate::client::resource::Client;
use crate::connection::events::{IterEntityDespawnEvent, IterEntitySpawnEvent};
use crate::protocol::component::ComponentProtocol;
use crate::protocol::message::MessageProtocol;
use crate::protocol::Protocol;

pub(crate) fn receive<P: Protocol>(world: &mut World) {
    trace!("Receive server packets");
    world.resource_scope(|world, mut client: Mut<Client<P>>| {
        world.resource_scope(|world, time: Mut<Time<Virtual>>| {
            let fixed_time = world.get_resource::<Time<Fixed>>().unwrap();

            // TODO: here we can control time elapsed from the client's perspective?

            // TODO: THE CLIENT COULD DO PHYSICS UPDATES INSIDE FIXED-UPDATE SYSTEMS
            //  WE SHOULD BE CALLING UPDATE INSIDE THOSE AS WELL SO THAT WE CAN SEND UPDATES
            //  IN THE MIDDLE OF THE FIXED UPDATE LOOPS
            //  WE JUST KEEP AN INTERNAL TIMER TO KNOW IF WE REACHED OUR TICK AND SHOULD RECEIVE/SEND OUT PACKETS?
            //  FIXED-UPDATE.expend() updates the clock by the fixed update interval
            //  THE NETWORK TICK INTERVAL COULD BE IN BETWEEN FIXED UPDATE INTERVALS

            // update client state, send keep-alives, receive packets from io
            // update connection sync state
            client.update(time.delta(), fixed_time.overstep()).unwrap();

            // buffer packets into message managers
            client.recv_packets().unwrap();
            // receive packets from message managers
            let mut events = client.receive(world);
            if !events.is_empty() {
                // NOTE: maybe no need to send those events, because the client knows when it's connected/disconnected?
                // if events.has_connection() {
                //     let mut connect_event_writer =
                //         world.get_resource_mut::<Events<ConnectEvent>>().unwrap();
                //     debug!("Client connected event");
                //     connect_event_writer.send(ConnectEvent::new(()));
                // }
                //
                // if events.has_disconnection() {
                //     let mut disconnect_event_writer =
                //         world.get_resource_mut::<Events<DisconnectEvent>>().unwrap();
                //     debug!("Client disconnected event");
                //     disconnect_event_writer.send(DisconnectEvent::new(()));
                // }

                // Message Events
                P::Message::push_message_events(world, &mut events);

                // SpawnEntity event
                if events.has_entity_spawn() {
                    let mut entity_spawn_event_writer = world
                        .get_resource_mut::<Events<EntitySpawnEvent>>()
                        .unwrap();
                    for (entity, _) in events.into_iter_entity_spawn() {
                        entity_spawn_event_writer.send(EntitySpawnEvent::new(entity, ()));
                    }
                }
                // DespawnEntity event
                if events.has_entity_despawn() {
                    let mut entity_despawn_event_writer = world
                        .get_resource_mut::<Events<EntityDespawnEvent>>()
                        .unwrap();
                    for (entity, _) in events.into_iter_entity_despawn() {
                        entity_despawn_event_writer.send(EntityDespawnEvent::new(entity, ()));
                    }
                }

                // Update component events (updates, inserts, removes)
                P::Components::push_component_events(world, &mut events);
            }
            trace!("finished recv");
        });
    });
}

pub(crate) fn send<P: Protocol>(mut client: ResMut<Client<P>>) {
    trace!("Send packets to server");
    // send buffered packets to io
    client.send_packets().unwrap();
}

pub(crate) fn is_ready_to_send<P: Protocol>(client: Res<Client<P>>) -> bool {
    client.is_ready_to_send()
}

pub(crate) fn sync_update<P: Protocol>(world: &mut World) {
    world.resource_scope(|world, mut client: Mut<Client<P>>| {
        world.resource_scope(|world, mut time: Mut<Time<Virtual>>| {
            // Handle pongs, update RTT estimates, update client's speed
            client.sync_update();
            // after the sync manager ran (and possibly re-computed RTT estimates), update the client's speed
            client.update_relative_speed(&mut time);
        })
    })
}
