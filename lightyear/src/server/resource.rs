//! Defines the server bevy resource
use bevy::utils::Duration;
use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::net::SocketAddr;

use anyhow::{Context, Result};
use bevy::ecs::component::Tick as BevyTick;
use bevy::ecs::entity::EntityHashMap;
use bevy::ecs::system::SystemParam;
use bevy::prelude::{Entity, Res, ResMut, Resource, World};
use bevy::utils::HashSet;
use crossbeam_channel::Sender;
use tracing::{debug, debug_span, error, info, trace, trace_span};

use crate::_reexport::FromType;
use crate::channel::builder::Channel;
use crate::connection::netcode::{generate_key, ClientId, ConnectToken};
use crate::connection::server::{NetServer, ServerConnection};
use crate::packet::message::Message;
use crate::prelude::PreSpawnedPlayerObject;
use crate::protocol::channel::ChannelKind;
use crate::protocol::Protocol;
use crate::server::room::{RoomId, RoomManager, RoomMut, RoomRef};
use crate::shared::replication::components::{NetworkTarget, Replicate, ReplicationGroupId};
use crate::shared::replication::components::{ShouldBeInterpolated, ShouldBePredicted};
use crate::shared::replication::ReplicationSend;
use crate::shared::tick_manager::Tick;
use crate::shared::tick_manager::TickManager;
use crate::shared::time_manager::TimeManager;
use crate::transport::io::Io;
use crate::transport::{PacketSender, Transport};

use super::config::ServerConfig;
use super::connection::ConnectionManager;
use super::events::ServerEvents;

#[derive(SystemParam)]
pub struct Server<'w, 's, P: Protocol> {
    // Config
    config: Res<'w, ServerConfig>,
    // Io
    io: Res<'w, Io>,
    // Netcode
    netcode: Res<'w, crate::connection::netcode::Server>,
    // Connections
    pub(crate) connection_manager: Res<'w, ConnectionManager<P>>,
    // Protocol
    pub protocol: Res<'w, P>,
    // Rooms
    pub(crate) room_manager: Res<'w, RoomManager>,
    // Time
    time_manager: Res<'w, TimeManager>,
    pub(crate) tick_manager: Res<'w, TickManager>,
    _marker: std::marker::PhantomData<&'s ()>,
}

#[derive(SystemParam)]
pub struct ServerMut<'w, 's, P: Protocol> {
    // Config
    config: ResMut<'w, ServerConfig>,
    // Netcode
    netcode: ResMut<'w, ServerConnection>,
    // Connections
    pub(crate) connection_manager: ResMut<'w, ConnectionManager<P>>,
    // Protocol
    pub protocol: ResMut<'w, P>,
    // Rooms
    pub(crate) room_manager: ResMut<'w, RoomManager>,
    // Time
    time_manager: ResMut<'w, TimeManager>,
    pub(crate) tick_manager: ResMut<'w, TickManager>,
    _marker: std::marker::PhantomData<&'s ()>,
}

impl<'w, 's, P: Protocol> ServerMut<'w, 's, P> {
    /// Return the server's received events since last frame
    pub(crate) fn events(&mut self) -> &mut ServerEvents<P> {
        &mut self.connection_manager.events
    }

    /// Update the server's internal state, queues up in a buffer any packets received from clients
    /// Sends keep-alive packets + any non-payload packet needed for netcode
    pub(crate) fn update(&mut self, delta: Duration) -> Result<()> {
        // update time manager
        self.time_manager.update(delta);

        // update netcode server
        self.netcode
            .try_update(delta.as_secs_f64())
            .context("Error updating netcode server")?;

        // update connections
        self.connection_manager
            .update(&self.time_manager, &self.tick_manager);

        // handle connection
        for client_id in self.netcode.new_connections().iter().copied() {
            // let client_addr = self.netcode.client_addr(client_id).unwrap();
            // info!("New connection from id: {}", client_id);
            self.connection_manager.add(client_id);
        }

        // handle disconnections
        for client_id in self.netcode.new_disconnections().iter().copied() {
            self.connection_manager.remove(client_id);
            self.room_manager.client_disconnect(client_id);
        }
        Ok(())
    }

    /// Receive packets from the transport layer and buffer them with the message manager
    pub(crate) fn recv_packets(&mut self) -> Result<()> {
        while let Some((mut reader, client_id)) = self.netcode.recv() {
            // TODO: use connection to apply on BOTH message manager and replication manager
            self.connection_manager
                .connection_mut(client_id)?
                .recv_packet(&mut reader, &self.tick_manager)?;
        }
        Ok(())
    }

    /// Receive messages from each connection, and update the events buffer
    pub(crate) fn receive(&mut self, world: &mut World) {
        self.connection_manager
            .receive(world, &self.time_manager, &self.tick_manager)
            .unwrap_or_else(|e| {
                error!("Error during receive: {}", e);
            });
    }

    // MESSAGES

    /// Queues up a message to be sent to all clients
    pub fn send_message_to_target<C: Channel, M: Message>(
        &mut self,
        message: M,
        target: NetworkTarget,
    ) -> Result<()>
    where
        M: Clone,
        P::Message: From<M>,
    {
        let _span =
            debug_span!("send_message", channel = ?C::type_name(), message = ?message.name(), ?target)
                .entered();
        self.connection_manager
            .buffer_message(message.into(), ChannelKind::of::<C>(), target)
    }

    /// Queues up a message to be sent to a client
    pub fn send_message<C: Channel, M: Message>(
        &mut self,
        client_id: ClientId,
        message: M,
    ) -> Result<()>
    where
        M: Clone,
        P::Message: From<M>,
    {
        self.send_message_to_target::<C, M>(message, NetworkTarget::Only(vec![client_id]))
    }

    // ROOM
    pub fn room_mut(&mut self, id: RoomId) -> RoomMut {
        RoomMut {
            id,
            manager: &mut self.room_manager,
        }
    }

    pub fn room(&self, id: RoomId) -> RoomRef {
        RoomRef {
            id,
            manager: &self.room_manager,
        }
    }
}

impl<'w, 's, P: Protocol> Server<'w, 's, P> {
    // /// Generate a connect token for a client with id `client_id`
    // pub fn token(&mut self, client_id: ClientId) -> ConnectToken {
    //     self.netcode
    //         .token(client_id, self.local_addr())
    //         .timeout_seconds(self.config.netcode.client_timeout_secs)
    //         .generate()
    //         .unwrap()
    // }

    pub fn local_addr(&self) -> SocketAddr {
        self.io.local_addr()
    }

    // IO

    pub fn io(&self) -> &Io {
        &self.io
    }

    // INPUTS

    // // TODO: exposed only for debugging
    // pub fn get_input_buffer(&self, client_id: ClientId) -> Option<&InputBuffer<P::Input>> {
    //     self.user_connections
    //         .get(&client_id)
    //         .map(|connection| &connection.input_buffer)
    // }
}

pub struct ServerContext {
    pub connections: crossbeam_channel::Receiver<ClientId>,
    pub disconnections: crossbeam_channel::Receiver<ClientId>,
}

impl<P: Protocol> ReplicationSend<P> for ConnectionManager<P> {
    fn update_priority(
        &mut self,
        replication_group_id: ReplicationGroupId,
        client_id: ClientId,
        priority: f32,
    ) -> Result<()> {
        debug!(
            ?client_id,
            ?replication_group_id,
            "Set priority to {:?}",
            priority
        );
        let replication_sender = &mut self.connection_mut(client_id)?.replication_sender;
        replication_sender.update_base_priority(replication_group_id, priority);
        Ok(())
    }

    fn new_connected_clients(&self) -> Vec<ClientId> {
        self.new_clients.clone()
    }

    fn prepare_entity_spawn(
        &mut self,
        entity: Entity,
        replicate: &Replicate<P>,
        target: NetworkTarget,
        system_current_tick: BevyTick,
    ) -> Result<()> {
        // debug!(?entity, "Spawning entity");
        let group_id = replicate.replication_group.group_id(Some(entity));
        // TODO: should we have additional state tracking so that we know we are in the process of sending this entity to clients?
        self.apply_replication(target).try_for_each(|client_id| {
            // trace!(
            //     ?client_id,
            //     ?entity,
            //     "Send entity spawn for tick {:?}",
            //     self.tick_manager.tick()
            // );
            let replication_sender = &mut self.connection_mut(client_id)?.replication_sender;
            // update the collect changes tick
            // replication_sender
            //     .group_channels
            //     .entry(group)
            //     .or_default()
            //     .update_collect_changes_since_this_tick(system_current_tick);
            replication_sender.prepare_entity_spawn(entity, group_id);
            // if we need to do prediction/interpolation, send a marker component to indicate that to the client
            if replicate.prediction_target.should_send_to(&client_id) {
                replication_sender.prepare_component_insert(
                    entity,
                    group_id,
                    P::Components::from(ShouldBePredicted::default()),
                );
            }
            if replicate.interpolation_target.should_send_to(&client_id) {
                replication_sender.prepare_component_insert(
                    entity,
                    group_id,
                    P::Components::from(ShouldBeInterpolated),
                );
            }
            // also set the priority for the group when we spawn it
            self.update_priority(group_id, client_id, replicate.replication_group.priority())?;

            Ok(())
        })
    }

    fn prepare_entity_despawn(
        &mut self,
        entity: Entity,
        replicate: &Replicate<P>,
        target: NetworkTarget,
        system_current_tick: BevyTick,
    ) -> Result<()> {
        let group_id = replicate.replication_group.group_id(Some(entity));
        self.apply_replication(target).try_for_each(|client_id| {
            // trace!(
            //     ?entity,
            //     ?client_id,
            //     "Send entity despawn for tick {:?}",
            //     self.tick_manager.tick()
            // );
            let replication_sender = &mut self.connection_mut(client_id)?.replication_sender;
            // update the collect changes tick
            // replication_sender
            //     .group_channels
            //     .entry(group)
            //     .or_default()
            //     .update_collect_changes_since_this_tick(system_current_tick);
            replication_sender.prepare_entity_despawn(entity, group_id);
            Ok(())
        })
    }

    // TODO: perf gain if we batch this? (send vec of components) (same for update/removes)
    fn prepare_component_insert(
        &mut self,
        entity: Entity,
        component: P::Components,
        replicate: &Replicate<P>,
        target: NetworkTarget,
        system_current_tick: BevyTick,
    ) -> Result<()> {
        let group_id = replicate.replication_group.group_id(Some(entity));
        let kind: P::ComponentKinds = (&component).into();

        // TODO: think about this. this feels a bit clumsy

        // handle ShouldBePredicted separately because of pre-spawning behaviour
        // Something to be careful of is this: let's say we receive on the server a pre-predicted entity with `ShouldBePredicted(1)`.
        // Then we rebroadcast it to other clients. If an entity `1` already exists on other clients; we will start using that entity
        //     as our Prediction target! That means that we should:
        // - even if pre-spawned replication, require users to set the `prediction_target` correctly
        //     - only broadcast `ShouldBePredicted` to the clients who have `prediction_target` set.
        // let should_be_predicted_kind =
        //     P::ComponentKinds::from(P::Components::from(ShouldBePredicted {
        //         client_entity: None,
        //     }));

        // same thing for PreSpawnedPlayerObject: that component should only be replicated to prediction_target
        let mut actual_target = target;
        if kind == <P::ComponentKinds as FromType<ShouldBePredicted>>::from_type()
            || kind == <P::ComponentKinds as FromType<PreSpawnedPlayerObject>>::from_type()
        {
            actual_target = replicate.prediction_target.clone();
        }

        self.apply_replication(actual_target)
            .try_for_each(|client_id| {
                // trace!(
                //     ?entity,
                //     component = ?kind,
                //     tick = ?self.tick_manager.tick(),
                //     "Inserting single component"
                // );
                let replication_sender = &mut self.connection_mut(client_id)?.replication_sender;
                // update the collect changes tick
                // replication_sender
                //     .group_channels
                //     .entry(group)
                //     .or_default()
                //     .update_collect_changes_since_this_tick(system_current_tick);
                replication_sender.prepare_component_insert(entity, group_id, component.clone());
                Ok(())
            })
    }

    fn prepare_component_remove(
        &mut self,
        entity: Entity,
        component_kind: P::ComponentKinds,
        replicate: &Replicate<P>,
        target: NetworkTarget,
        system_current_tick: BevyTick,
    ) -> Result<()> {
        let group_id = replicate.replication_group.group_id(Some(entity));
        debug!(?entity, ?component_kind, "Sending RemoveComponent");
        self.apply_replication(target).try_for_each(|client_id| {
            let replication_sender = &mut self.connection_mut(client_id)?.replication_sender;
            // TODO: I don't think it's actually correct to only correct the changes since that action.
            // what if we do:
            // - Frame 1: update is ACKED
            // - Frame 2: update
            // - Frame 3: action
            // - Frame 4: send
            // then we won't send the frame-2 update because we only collect changes since frame 3
            // replication_sender
            //     .group_channels
            //     .entry(group)
            //     .or_default()
            //     .update_collect_changes_since_this_tick(system_current_tick);
            replication_sender.prepare_component_remove(entity, group_id, component_kind);
            Ok(())
        })
    }

    fn prepare_entity_update(
        &mut self,
        entity: Entity,
        component: P::Components,
        replicate: &Replicate<P>,
        target: NetworkTarget,
        component_change_tick: BevyTick,
        system_current_tick: BevyTick,
    ) -> Result<()> {
        let kind: P::ComponentKinds = (&component).into();
        trace!(
            ?kind,
            ?entity,
            ?component_change_tick,
            ?system_current_tick,
            "Prepare entity update"
        );

        let group_id = replicate.group_id(Some(entity));
        self.apply_replication(target).try_for_each(|client_id| {
            // TODO: should we have additional state tracking so that we know we are in the process of sending this entity to clients?
            let replication_sender = &mut self.connection_mut(client_id)?.replication_sender;
            let collect_changes_since_this_tick = replication_sender
                .group_channels
                .entry(group_id)
                .or_default()
                .collect_changes_since_this_tick;
            // send the update for all changes newer than the last ack bevy tick for the group
            debug!(
                ?kind,
                change_tick = ?component_change_tick,
                ?collect_changes_since_this_tick,
                "prepare entity update changed check (we want the component-change-tick to be higher than collect-changes-since-this-tick)"
            );

            if collect_changes_since_this_tick.map_or(true, |tick| {
                component_change_tick.is_newer_than(tick, system_current_tick)
            }) {
                trace!(
                    change_tick = ?component_change_tick,
                    ?collect_changes_since_this_tick,
                    current_tick = ?system_current_tick,
                    "prepare entity update changed check"
                );
                // trace!(
                //     ?entity,
                //     component = ?kind,
                //     tick = ?self.tick_manager.tick(),
                //     "Updating single component"
                // );
                replication_sender.prepare_entity_update(entity, group_id, component.clone());
            }
            Ok(())
        })
    }

    /// Buffer the replication messages
    fn buffer_replication_messages(&mut self, tick: Tick, bevy_tick: BevyTick) -> Result<()> {
        self.buffer_replication_messages(tick, bevy_tick)
    }

    fn get_mut_replicate_component_cache(&mut self) -> &mut EntityHashMap<Replicate<P>> {
        &mut self.replicate_component_cache
    }
}
