//! Defines the client bevy resource
use std::net::SocketAddr;

use anyhow::Result;
use bevy::ecs::component::Tick as BevyTick;
use bevy::ecs::system::SystemParam;
use bevy::prelude::{Entity, Res, ResMut, Resource, World};
use bevy::utils::Duration;
use bevy::utils::EntityHashMap;
use tracing::{debug, trace, trace_span};

use crate::_reexport::ReplicationSend;
use crate::channel::builder::Channel;
use crate::connection::client::{ClientConnection, NetClient};
use crate::connection::netcode::ClientId;
use crate::connection::netcode::{ConnectToken, Key};
use crate::inputs::native::input_buffer::InputBuffer;
use crate::packet::message::Message;
use crate::prelude::NetworkTarget;
use crate::protocol::channel::ChannelKind;
use crate::protocol::Protocol;
use crate::shared::events::ConnectionEvents;
use crate::shared::replication::components::{Replicate, ReplicationGroupId};
use crate::shared::replication::receive::ReplicationReceiver;
use crate::shared::replication::send::ReplicationSender;
use crate::shared::tick_manager::Tick;
use crate::shared::tick_manager::TickManager;
use crate::shared::time_manager::TimeManager;
use crate::transport::PacketSender;

use super::config::ClientConfig;
use super::connection::ConnectionManager;

#[derive(SystemParam)]
pub struct Client<'w, 's, P: Protocol> {
    //config
    config: Res<'w, ClientConfig>,
    // netcode
    netcode: Res<'w, ClientConnection>,
    // connection
    pub(crate) connection: Res<'w, ConnectionManager<P>>,
    // protocol
    protocol: Res<'w, P>,
    // events
    events: Res<'w, ConnectionEvents<P>>,
    // syncing
    pub(crate) time_manager: Res<'w, TimeManager>,
    pub(crate) tick_manager: Res<'w, TickManager>,
    _marker: std::marker::PhantomData<&'s ()>,
}

#[derive(SystemParam)]
pub struct ClientMut<'w, 's, P: Protocol> {
    //config
    config: ResMut<'w, ClientConfig>,
    // netcode
    netcode: ResMut<'w, ClientConnection>,
    // connection
    pub(crate) connection: ResMut<'w, ConnectionManager<P>>,
    // protocol
    protocol: ResMut<'w, P>,
    // events
    events: ResMut<'w, ConnectionEvents<P>>,
    // syncing
    pub(crate) time_manager: ResMut<'w, TimeManager>,
    pub(crate) tick_manager: ResMut<'w, TickManager>,
    _marker: std::marker::PhantomData<&'s ()>,
}

impl<'w, 's, P: Protocol> ClientMut<'w, 's, P> {
    /// Maintain connection with server, queues up any packet received from the server
    pub(crate) fn update(&mut self, delta: Duration) -> Result<()> {
        self.time_manager.update(delta);
        self.netcode.try_update(delta.as_secs_f64())?;

        // only start the connection (sending messages, sending pings, starting sync, etc.)
        // once we are connected
        if self.netcode.is_connected() {
            self.connection
                .update(&self.time_manager, &self.tick_manager);
        }

        Ok(())
    }

    /// Receive messages from the server
    pub(crate) fn receive(&mut self, world: &mut World) -> ConnectionEvents<P> {
        trace!("Receive server packets");
        self.connection
            .receive(world, &self.time_manager, &self.tick_manager)
    }

    /// Receive packets from the transport layer and buffer them with the message manager
    pub(crate) fn recv_packets(&mut self) -> Result<()> {
        while let Some(mut reader) = self.netcode.recv() {
            self.connection
                .recv_packet(&mut reader, &self.tick_manager)?;
        }
        Ok(())
    }

    // NETCODE

    /// Start the connection process with the server
    pub fn connect(&mut self) -> Result<()> {
        self.netcode.connect()
    }

    // MESSAGES

    // TODO: i'm not event sure that is something we want.
    //  it could open the door to the client flooding other players with messages
    //  maybe we should let users re-broadcast messages from the server themselves instead of using this
    //  Also it would make the code much simpler by having a single `ProtocolMessage` enum
    //  instead of `ClientMessage` and `ServerMessage`
    /// Send a message to the server, the message should be re-broadcasted according to the `target`
    pub fn send_message_to_target<C: Channel, M: Message>(
        &mut self,
        message: M,
        target: NetworkTarget,
    ) -> Result<()>
    where
        P::Message: From<M>,
    {
        let channel = ChannelKind::of::<C>();
        self.connection
            .buffer_message(message.into(), channel, target)
    }

    /// Send a message to the server
    pub fn send_message<C: Channel, M: Message>(&mut self, message: M) -> Result<()>
    where
        P::Message: From<M>,
    {
        let channel = ChannelKind::of::<C>();
        self.connection
            .buffer_message(message.into(), channel, NetworkTarget::None)
    }

    // INPUTS

    // TODO: maybe put the input_buffer directly in Client ?
    //  layer of indirection feelds annoying
    pub fn add_input(&mut self, input: P::Input) {
        self.connection.add_input(input, self.tick_manager.tick());
    }
}

#[derive(Resource, Default, Clone)]
#[allow(clippy::large_enum_variant)]
/// Struct used to authenticate with the server
pub enum Authentication {
    /// Use a `ConnectToken` that was already received (usually from a secure-connection to a webserver)
    Token(ConnectToken),
    /// Or build a `ConnectToken` manually from the given parameters
    Manual {
        server_addr: SocketAddr,
        client_id: u64,
        private_key: Key,
        protocol_id: u64,
    },
    #[default]
    /// Request a connect token from the backend
    RequestConnectToken,
}

impl Authentication {
    pub(crate) fn get_token(self, client_timeout_secs: i32) -> Option<ConnectToken> {
        match self {
            Authentication::Token(token) => Some(token),
            Authentication::Manual {
                server_addr,
                client_id,
                private_key,
                protocol_id,
            } => ConnectToken::build(server_addr, protocol_id, client_id, private_key)
                .timeout_seconds(client_timeout_secs)
                .generate()
                .ok(),
            Authentication::RequestConnectToken { .. } => None,
        }
    }
}

impl<'w, 's, P: Protocol> Client<'w, 's, P> {
    pub fn config(&self) -> &ClientConfig {
        &self.config
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.netcode.local_addr()
    }

    // NETCODE

    pub fn is_connected(&self) -> bool {
        self.netcode.is_connected()
    }

    /// Returns true if the client is connected and has been time-synced with the server
    pub fn is_synced(&self) -> bool {
        self.connection.sync_manager.is_synced()
    }

    /// Returns the client id assigned by the server
    pub fn id(&self) -> ClientId {
        self.netcode.id()
    }

    // IO

    // pub fn io(&self) -> &Io {
    //     &self.io
    // }

    // REPLICATION
    pub(crate) fn replication_sender(&self) -> &ReplicationSender<P> {
        &self.connection.replication_sender
    }

    pub(crate) fn replication_receiver(&self) -> &ReplicationReceiver<P> {
        &self.connection.replication_receiver
    }
}

// INPUT
impl<'w, 's, P: Protocol> Client<'w, 's, P> {
    pub fn get_input_buffer(&self) -> &InputBuffer<P::Input> {
        &self.connection.input_buffer
    }
}

// Access some internals for tests
#[cfg(test)]
impl<'w, 's, P: Protocol> Client<'w, 's, P> {
    // pub fn set_latest_received_server_tick(&mut self, tick: Tick) {
    //     self.connection.sync_manager.latest_received_server_tick = Some(tick);
    //     self.connection
    //         .sync_manager
    //         .duration_since_latest_received_server_tick = Duration::default();
    // }

    pub fn connection(&self) -> &ConnectionManager<P> {
        &self.connection
    }

    // pub fn set_synced(&mut self) {
    //     self.connection.sync_manager.synced = true;
    // }
}

impl<P: Protocol> ReplicationSend<P> for ConnectionManager<P> {
    fn update_priority(
        &mut self,
        replication_group_id: ReplicationGroupId,
        client_id: ClientId,
        priority: f32,
    ) -> Result<()> {
        self.replication_sender
            .update_base_priority(replication_group_id, priority);
        Ok(())
    }

    fn new_connected_clients(&self) -> Vec<ClientId> {
        vec![]
    }

    fn prepare_entity_spawn(
        &mut self,
        entity: Entity,
        replicate: &Replicate<P>,
        target: NetworkTarget,
        system_current_tick: BevyTick,
    ) -> Result<()> {
        // trace!(?entity, "Send entity spawn for tick {:?}", self.tick());
        let group_id = replicate.replication_group.group_id(Some(entity));
        let replication_sender = &mut self.replication_sender;
        // update the collect changes tick
        // (we can collect changes only since the last actions because all updates will wait for that action to be spawned)
        // TODO: I don't think it's correct to update the change-tick since the latest action!
        // replication_sender
        //     .group_channels
        //     .entry(group)
        //     .or_default()
        //     .update_collect_changes_since_this_tick(system_current_tick);
        replication_sender.prepare_entity_spawn(entity, group_id);

        // also set the priority for the group when we spawn it
        self.update_priority(
            group_id,
            // the client id argument is ignored on the client
            0,
            replicate.replication_group.priority(),
        )?;
        // Prediction/interpolation
        Ok(())
    }

    fn prepare_entity_despawn(
        &mut self,
        entity: Entity,
        replicate: &Replicate<P>,
        target: NetworkTarget,
        system_current_tick: BevyTick,
    ) -> Result<()> {
        // trace!(?entity, "Send entity despawn for tick {:?}", self.tick());
        let group_id = replicate.replication_group.group_id(Some(entity));
        let replication_sender = &mut self.replication_sender;
        // update the collect changes tick
        // replication_sender
        //     .group_channels
        //     .entry(group)
        //     .or_default()
        //     .update_collect_changes_since_this_tick(system_current_tick);
        replication_sender.prepare_entity_despawn(entity, group_id);
        // Prediction/interpolation
        Ok(())
    }

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
        // debug!(
        //     ?entity,
        //     component = ?kind,
        //     tick = ?self.tick_manager.tick(),
        //     "Inserting single component"
        // );
        // update the collect changes tick
        // self.replication_sender
        //     .group_channels
        //     .entry(group)
        //     .or_default()
        //     .update_collect_changes_since_this_tick(system_current_tick);
        self.replication_sender
            .prepare_component_insert(entity, group_id, component.clone());
        Ok(())
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
        // self.replication_sender
        //     .group_channels
        //     .entry(group)
        //     .or_default()
        //     .update_collect_changes_since_this_tick(system_current_tick);
        self.replication_sender
            .prepare_component_remove(entity, group_id, component_kind);
        Ok(())
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
        let group_id = replicate.group_id(Some(entity));
        // TODO: should we have additional state tracking so that we know we are in the process of sending this entity to clients?
        let collect_changes_since_this_tick = self
            .replication_sender
            .group_channels
            .entry(group_id)
            .or_default()
            .collect_changes_since_this_tick;
        // send the update for all changes newer than the last ack bevy tick for the group

        if collect_changes_since_this_tick.map_or(true, |c| {
            component_change_tick.is_newer_than(c, system_current_tick)
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
            self.replication_sender
                .prepare_entity_update(entity, group_id, component.clone());
        }
        Ok(())
    }

    fn buffer_replication_messages(&mut self, tick: Tick, bevy_tick: BevyTick) -> Result<()> {
        let _span = trace_span!("buffer_replication_messages").entered();
        self.buffer_replication_messages(tick, bevy_tick)
    }
    fn get_mut_replicate_component_cache(&mut self) -> &mut EntityHashMap<Entity, Replicate<P>> {
        &mut self.replication_sender.replicate_component_cache
    }
}

// Functions related to Interpolation (maybe make it a trait)?
impl<'w, 's, P: Protocol> Client<'w, 's, P> {
    pub fn interpolation_tick(&self) -> Tick {
        self.connection
            .sync_manager
            .interpolation_tick(&self.tick_manager)
    }
    // // TODO: how to mock this in tests?
    // // TODO: actually we shouldn't use interpolation ticks, but use times directly, so we can take into account the overstep properly?
    // pub(crate) fn interpolated_tick(&mut self) -> Tick {
    //     self.connection
    //         .sync_manager
    //         .update_estimated_interpolated_tick(
    //             &self.config.interpolation.delay,
    //             &self.tick_manager,
    //             &self.time_manager,
    //         );
    //     self.connection.sync_manager.estimated_interpolation_tick
    // }
}
