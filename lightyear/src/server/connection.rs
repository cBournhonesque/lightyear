//! Specify how a Server sends/receives messages with a Client
#[cfg(not(feature = "std"))]
use alloc::{boxed::Box, vec, vec::Vec};
use bevy::ecs::component::Tick as BevyTick;
use bevy::ecs::entity::MapEntities;
use bevy::platform::collections::hash_map::{Entry, HashMap};
use bevy::prelude::{Component, Entity, Resource, World};
use bevy::ptr::Ptr;
use bytes::Bytes;
use core::time::Duration;
use tracing::{debug, info, info_span, trace, trace_span};
#[cfg(feature = "trace")]
use tracing::{instrument, Level};

use crate::channel::builder::{
    EntityActionsChannel, EntityUpdatesChannel, PingChannel, PongChannel,
};

use crate::channel::receivers::ChannelReceive;
use crate::channel::senders::ChannelSend;
use crate::client::message::ClientMessage;
use crate::connection::id::ClientId;
use crate::connection::netcode::MAX_PACKET_SIZE;
use crate::packet::message_manager::MessageManager;
use crate::packet::packet_builder::{Payload, RecvPayload};
use crate::prelude::server::DisconnectEvent;
use crate::prelude::{
    ChannelKind, Message, PreSpawned, ReplicationConfig, ReplicationGroup, ShouldBePredicted,
};
use crate::protocol::channel::ChannelRegistry;
use crate::protocol::component::{
    registry::ComponentRegistry, ComponentError, ComponentKind, ComponentNetId,
};
use crate::protocol::message::registry::MessageRegistry;
use crate::protocol::message::MessageError;
use crate::protocol::registry::NetId;
use crate::serialize::reader::Reader;
use crate::serialize::writer::Writer;
use crate::serialize::{SerializationError, ToBytes};
use crate::server::config::PacketConfig;
use crate::server::error::ServerError;
use crate::server::events::{ConnectEvent, ServerEvents};
use crate::shared::events::connection::ConnectionEvents;
use crate::shared::ping::manager::{PingConfig, PingManager};
use crate::shared::ping::message::{Ping, Pong};
use crate::shared::replication::components::ReplicationGroupId;
use crate::shared::replication::delta::DeltaManager;
use crate::shared::replication::entity_map::SendEntityMap;
use crate::shared::replication::network_target::NetworkTarget;
use crate::shared::replication::receive::ReplicationReceiver;
use crate::shared::replication::send::ReplicationSender;
use crate::shared::replication::{EntityActionsMessage, EntityUpdatesMessage, ReplicationPeer};
use crate::shared::replication::{ReplicationReceive, ReplicationSend};
use crate::shared::sets::ServerMarker;
use crate::shared::tick_manager::Tick;
use crate::shared::tick_manager::TickManager;
use crate::shared::time_manager::TimeManager;

#[derive(Resource)]
pub struct ConnectionManager {
    pub(crate) connections: HashMap<ClientId, Connection>,
    pub(crate) message_registry: MessageRegistry,
    channel_registry: ChannelRegistry,
    pub(crate) events: ServerEvents,
    pub delta_manager: DeltaManager,

    // list of clients that connected since the last time we sent replication messages
    // (we want to keep track of them because we need to replicate the entire world state to them)
    pub(crate) new_clients: Vec<ClientId>,
    pub(crate) writer: Writer,

    // CONFIG
    replication_config: ReplicationConfig,
    packet_config: PacketConfig,
    ping_config: PingConfig,
}

// This is useful in cases where we need to temporarily store a fake ConnectionManager
impl Default for ConnectionManager {
    fn default() -> Self {
        Self::new(
            MessageRegistry::default(),
            ChannelRegistry::default(),
            ReplicationConfig::default(),
            PacketConfig::default(),
            PingConfig::default(),
        )
    }
}

impl ConnectionManager {
    pub(crate) fn new(
        message_registry: MessageRegistry,
        channel_registry: ChannelRegistry,
        replication_config: ReplicationConfig,
        packet_config: PacketConfig,
        ping_config: PingConfig,
    ) -> Self {
        Self {
            connections: HashMap::default(),
            message_registry,
            channel_registry,
            events: ServerEvents::new(),
            delta_manager: DeltaManager::default(),
            new_clients: vec![],
            writer: Writer::with_capacity(MAX_PACKET_SIZE),
            replication_config,
            packet_config,
            ping_config,
        }
    }

    /// Return the [`Entity`] associated with the given [`ClientId`]
    pub fn client_entity(&self, client_id: ClientId) -> Result<Entity, ServerError> {
        self.connection(client_id).map(|c| c.entity)
    }

    /// Return the list of connected [`ClientId`]s
    pub fn connected_clients(&self) -> impl Iterator<Item = ClientId> + '_ {
        self.connections.keys().copied()
    }

    // TODO: we need `&mut self` because MapEntities requires `&mut EntityMapper` even though it's not needed here
    /// Convert entities in the message to be compatible with the remote world of the provided client
    pub fn map_entities_to_remote<M: Message + MapEntities>(
        &mut self,
        message: &mut M,
        client_id: ClientId,
    ) -> Result<(), ServerError> {
        let mapper = &mut self
            .connection_mut(client_id)?
            .replication_receiver
            .remote_entity_map
            .local_to_remote;
        message.map_entities(mapper);
        Ok(())
    }

    /// Update the priority of a `ReplicationGroup` that is replicated to a given client
    pub fn update_priority(
        &mut self,
        replication_group_id: ReplicationGroupId,
        client_id: ClientId,
        priority: f32,
    ) -> Result<(), ServerError> {
        debug!(
            ?client_id,
            ?replication_group_id,
            "Set priority to {:?}",
            priority
        );
        self.connection_mut(client_id)?
            .replication_sender
            .update_base_priority(replication_group_id, priority);
        Ok(())
    }

    /// Find the list of connected clients that match the provided [`NetworkTarget`]
    pub(crate) fn connected_targets<'a: 'b, 'b>(
        &'a self,
        target: &'b NetworkTarget,
    ) -> Box<dyn Iterator<Item = &'a Connection> + 'b> {
        // TODO: avoid extra allocations ... maybe by putting the list of connected clients in a separate resource?
        match target {
            NetworkTarget::All => Box::new(self.connections.values()),
            NetworkTarget::AllExceptSingle(client_id) => Box::new(
                self.connections
                    .values()
                    .filter(move |c| c.client_id != *client_id),
            ),
            NetworkTarget::AllExcept(client_ids) => Box::new(
                self.connections
                    .values()
                    .filter(move |c| !client_ids.contains(&c.client_id)),
            ),
            NetworkTarget::Single(client_id) => {
                Box::new(self.connections.get(client_id).into_iter())
            }
            NetworkTarget::Only(client_ids) => Box::new(
                self.connections
                    .values()
                    .filter(move |c| client_ids.contains(&c.client_id)),
            ),
            NetworkTarget::None => Box::new(core::iter::empty()),
        }
    }

    pub fn connection(&self, client_id: ClientId) -> Result<&Connection, ServerError> {
        self.connections
            .get(&client_id)
            .ok_or(ServerError::ClientIdNotFound(client_id))
    }

    pub fn connection_mut(&mut self, client_id: ClientId) -> Result<&mut Connection, ServerError> {
        self.connections
            .get_mut(&client_id)
            .ok_or(ServerError::ClientIdNotFound(client_id))
    }

    pub(crate) fn update(
        &mut self,
        world_tick: BevyTick,
        time_manager: &TimeManager,
        tick_manager: &TickManager,
    ) {
        self.connections.values_mut().for_each(|connection| {
            connection.update(world_tick, time_manager, tick_manager);
        });
    }

    /// Add a new [`Connection`] to the list of connections with the given [`ClientId`]
    pub(crate) fn add(&mut self, client_id: ClientId, client_entity: Entity) {
        match self.connections.entry(client_id) { Entry::Vacant(e) => {
            #[cfg(feature = "metrics")]
            metrics::gauge!("server::connected_clients").increment(1.0);

            info!("New connection from id: {}", client_id);
            let connection = Connection::new(
                client_id,
                client_entity,
                &self.channel_registry,
                self.replication_config,
                self.packet_config,
                self.ping_config,
            );
            self.events.add_connect_event(ConnectEvent {
                client_id,
                entity: client_entity,
            });
            self.new_clients.push(client_id);
            e.insert(connection);
        } _ => {
            info!("Client {} was already in the connections list", client_id);
        }}
    }

    /// Remove the connection associated with the given [`ClientId`]
    ///
    /// Emits a server [`DisconnectEvent`].
    pub(crate) fn remove(&mut self, client_id: ClientId) {
        if let Ok(entity) = self.client_entity(client_id) {
            debug!("Sending Client DisconnectEvent");
            self.events
                .add_disconnect_event(DisconnectEvent { client_id, entity });
        }
        if self.connections.remove(&client_id).is_some() {
            #[cfg(feature = "metrics")]
            metrics::gauge!("server::connected_clients").decrement(1.0);
            info!("Client {} disconnected", client_id);
        };
    }

    /// Buffer all the replication messages to send.
    /// Keep track of the bevy Change Tick: when a message is acked, we know that we only have to send
    /// the updates since that Change Tick
    #[cfg_attr(feature = "trace", instrument(level = Level::INFO, skip_all))]
    pub(crate) fn buffer_replication_messages(
        &mut self,
        tick: Tick,
        bevy_tick: BevyTick,
        time_manager: &TimeManager,
    ) -> Result<(), ServerError> {
        let _span = info_span!("buffer_replication_messages").entered();
        self.connections
            .values_mut()
            .try_for_each(move |c| c.buffer_replication_messages(tick, bevy_tick, time_manager))
    }

    #[cfg_attr(feature = "trace", instrument(level = Level::INFO, skip_all))]
    pub(crate) fn receive(
        &mut self,
        world: &mut World,
        component_registry: &mut ComponentRegistry,
        message_registry: &MessageRegistry,
        time_manager: &TimeManager,
        tick_manager: &TickManager,
    ) -> Result<(), ServerError> {
        let mut messages_to_rebroadcast = vec![];
        // TODO: do this in parallel
        self.connections
            .iter_mut()
            .try_for_each(|(client_id, connection)| {
                let _span = trace_span!("receive", ?client_id).entered();
                // receive events on the connection
                let events = connection.receive(
                    world,
                    component_registry,
                    message_registry,
                    time_manager,
                    tick_manager,
                )?;
                // move the events from the connection to the connection manager
                self.events.push_events(*client_id, events);

                // rebroadcast messages
                messages_to_rebroadcast
                    .extend(core::mem::take(&mut connection.messages_to_rebroadcast));
                Ok::<(), ServerError>(())
            })?;
        for (message, target, channel_kind) in messages_to_rebroadcast {
            self.buffer_message_bytes(message, channel_kind, target)?;
        }
        Ok(())
    }
}

impl ConnectionManager {
    /// Helper function to prepare component insert for components for which we know the type
    pub(crate) fn prepare_typed_component_insert<C: Component>(
        &mut self,
        entity: Entity,
        group_id: ReplicationGroupId,
        client_id: ClientId,
        component_registry: &ComponentRegistry,
        data: &mut C,
    ) -> Result<(), ServerError> {
        let net_id = component_registry
            .get_net_id::<C>()
            .ok_or::<ServerError>(ComponentError::NotRegistered.into())?;
        // TODO: add SendEntityMap here!
        // We store the Bytes in a hashmap, maybe more efficient to write the replication message directly?
        component_registry.serialize(data, &mut self.writer, &mut SendEntityMap::default())?;
        let raw_data = self.writer.split();
        self.connection_mut(client_id)?
            .replication_sender
            .prepare_component_insert(entity, group_id, raw_data);
        Ok(())
    }
}

/// Find the list of connected clients that match the provided [`NetworkTarget`]
pub(crate) fn connected_targets_mut<'a: 'b, 'b>(
    connections: &'a mut HashMap<ClientId, Connection>,
    target: &'b NetworkTarget,
) -> Box<dyn Iterator<Item = &'a mut Connection> + 'b> {
    // TODO: avoid extra allocations ... maybe by putting the list of connected clients in a separate resource?
    match target {
        NetworkTarget::All => Box::new(connections.values_mut()),
        NetworkTarget::AllExceptSingle(client_id) => Box::new(
            connections
                .values_mut()
                .filter(move |c| c.client_id != *client_id),
        ),
        NetworkTarget::AllExcept(client_ids) => Box::new(
            connections
                .values_mut()
                .filter(move |c| !client_ids.contains(&c.client_id)),
        ),
        NetworkTarget::Single(client_id) => Box::new(connections.get_mut(client_id).into_iter()),
        NetworkTarget::Only(client_ids) => Box::new(
            connections
                .values_mut()
                .filter(move |c| client_ids.contains(&c.client_id)),
        ),
        NetworkTarget::None => Box::new(core::iter::empty()),
    }
}

/// Wrapper that handles the connection between the server and a client
pub struct Connection {
    pub(crate) client_id: ClientId,
    /// We create one entity per connected client, so that users
    /// can store metadata about the client using the ECS
    pub(crate) entity: Entity,
    pub message_manager: MessageManager,
    pub(crate) replication_sender: ReplicationSender,
    pub replication_receiver: ReplicationReceiver,
    pub(crate) events: ConnectionEvents,
    pub(crate) ping_manager: PingManager,

    // TODO: maybe don't do any replication until connection is synced?
    /// Used to transfer raw bytes to a system that can convert the bytes to the actual type
    pub(crate) received_messages: Vec<(Bytes, NetworkTarget, ChannelKind)>,
    pub(crate) received_input_messages: HashMap<NetId, Vec<(Bytes, NetworkTarget, ChannelKind)>>,
    #[cfg(feature = "leafwing")]
    pub(crate) received_leafwing_input_messages:
        HashMap<NetId, Vec<(Bytes, NetworkTarget, ChannelKind)>>,
    pub(crate) writer: Writer,
    // messages that we have received that need to be rebroadcasted to other clients
    pub(crate) messages_to_rebroadcast: Vec<(Bytes, NetworkTarget, ChannelKind)>,
    /// True if this connection corresponds to a local client when running in host-server mode
    is_local_client: bool,
    /// Messages to send to the local client (we don't buffer them in the MessageManager because there is no io)
    pub(crate) local_messages_to_send: Vec<Bytes>,
}

impl Connection {
    pub(crate) fn new(
        client_id: ClientId,
        entity: Entity,
        channel_registry: &ChannelRegistry,
        replication_config: ReplicationConfig,
        packet_config: PacketConfig,
        ping_config: PingConfig,
    ) -> Self {
        let bandwidth_cap_enabled = packet_config.bandwidth_cap_enabled;
        // create the message manager and the channels
        let mut message_manager = MessageManager::new(
            channel_registry,
            packet_config.nack_rtt_multiple,
            packet_config.into(),
        );
        // get notified about acks/nacks for replication-update messages
        let entity_updates_sender = &mut message_manager
            .channels
            .get_mut(&ChannelKind::of::<EntityUpdatesChannel>())
            .unwrap()
            .sender;
        let update_nacks_receiver = entity_updates_sender.subscribe_nacks();
        let update_acks_receiver = entity_updates_sender.subscribe_acks();
        // get a channel to get notified when a replication update message gets actually send (to update priority)
        let replication_update_send_receiver =
            message_manager.get_replication_update_send_receiver();
        let replication_sender = ReplicationSender::new(
            update_acks_receiver,
            update_nacks_receiver,
            replication_update_send_receiver,
            replication_config,
            bandwidth_cap_enabled,
        );
        let replication_receiver = ReplicationReceiver::new();
        Self {
            client_id,
            entity,
            message_manager,
            replication_sender,
            replication_receiver,
            ping_manager: PingManager::new(ping_config),
            events: ConnectionEvents::default(),
            received_messages: Vec::default(),
            received_input_messages: HashMap::default(),
            #[cfg(feature = "leafwing")]
            received_leafwing_input_messages: HashMap::default(),
            writer: Writer::with_capacity(MAX_PACKET_SIZE),
            messages_to_rebroadcast: vec![],
            is_local_client: false,
            local_messages_to_send: vec![],
        }
    }

    /// Update the connection to make clear that it corresponds to the local client
    pub(crate) fn set_local_client(&mut self) {
        self.is_local_client = true;
    }

    /// Returns true if this connection corresponds to the local client in HostServer mode
    pub(crate) fn is_local_client(&self) -> bool {
        self.is_local_client
    }

    /// Return the latest estimate of rtt
    pub fn rtt(&self) -> Duration {
        self.ping_manager.rtt()
    }

    /// Return the latest estimate of jitter
    pub fn jitter(&self) -> Duration {
        self.ping_manager.jitter()
    }

    pub(crate) fn update(
        &mut self,
        world_tick: BevyTick,
        time_manager: &TimeManager,
        tick_manager: &TickManager,
    ) {
        if self.is_local_client() {
            return;
        }
        self.message_manager
            .update(time_manager, &self.ping_manager, tick_manager);
        self.replication_sender.update(world_tick);
        self.ping_manager.update(time_manager);
    }

    pub(crate) fn buffer_message(
        &mut self,
        message: Bytes,
        channel: ChannelKind,
    ) -> Result<(), ServerError> {
        // TODO: i know channel names never change so i should be able to get them as static
        // TODO: just have a channel registry enum as well?
        let channel_name = self
            .message_manager
            .channel_registry
            .name(&channel)
            .ok_or::<ServerError>(MessageError::NotRegistered.into())?;
        // message.emit_send_logs(&channel_name);
        self.message_manager.buffer_send(message, channel)?;
        Ok(())
    }

    #[cfg_attr(feature = "trace", instrument(level = Level::INFO, skip_all))]
    pub(crate) fn buffer_replication_messages(
        &mut self,
        tick: Tick,
        bevy_tick: BevyTick,
        time_manager: &TimeManager,
    ) -> Result<(), ServerError> {
        self.replication_sender.accumulate_priority(time_manager);
        self.replication_sender.send_actions_messages(
            tick,
            bevy_tick,
            &mut self.writer,
            &mut self.message_manager,
        )?;
        self.replication_sender.send_updates_messages(
            tick,
            bevy_tick,
            &mut self.writer,
            &mut self.message_manager,
        )?;
        Ok(())
    }

    fn send_ping(&mut self, ping: Ping) -> Result<(), ServerError> {
        trace!("Sending ping {:?}", ping);
        ping.to_bytes(&mut self.writer)?;
        let message_bytes = self.writer.split();
        self.message_manager
            .buffer_send(message_bytes, ChannelKind::of::<PingChannel>())?;
        Ok(())
    }

    fn send_pong(&mut self, pong: Pong) -> Result<(), ServerError> {
        trace!("Sending pong {:?}", pong);
        pong.to_bytes(&mut self.writer)?;
        let message_bytes = self.writer.split();
        self.message_manager
            .buffer_send(message_bytes, ChannelKind::of::<PongChannel>())?;
        Ok(())
    }

    /// Send packets that are ready to be sent
    pub fn send_packets(
        &mut self,
        time_manager: &TimeManager,
        tick_manager: &TickManager,
    ) -> Result<Vec<Payload>, ServerError> {
        // update the ping manager with the actual send time
        // TODO: issues here: we would like to send the ping/pong messages immediately, otherwise the recorded current time is incorrect
        //   - can give infinity priority to this channel?
        //   - can write directly to io otherwise?

        // maybe send pings
        // same thing, we want the correct send time for the ping
        // (and not have the delay between when we prepare the ping and when we send the packet)
        if let Some(ping) = self.ping_manager.maybe_prepare_ping(time_manager) {
            self.send_ping(ping)?;
        }

        // prepare the pong messages with the correct send time
        self.ping_manager
            .take_pending_pongs()
            .into_iter()
            .try_for_each(|mut pong| {
                trace!("Sending pong {:?}", pong);
                // update the send time of the pong
                pong.pong_sent_time = time_manager.current_time();
                self.send_pong(pong)?;
                Ok::<(), ServerError>(())
            })?;
        let payloads = self.message_manager.send_packets(tick_manager.tick())?;

        // update the replication sender about which messages were actually sent, and accumulate priority
        self.replication_sender.recv_send_notification();
        Ok(payloads)
    }

    pub fn receive(
        &mut self,
        world: &mut World,
        component_registry: &mut ComponentRegistry,
        message_registry: &MessageRegistry,
        time_manager: &TimeManager,
        tick_manager: &TickManager,
    ) -> Result<ConnectionEvents, ServerError> {
        let _span = trace_span!("receive").entered();
        self.message_manager
            .channels
            .iter_mut()
            .try_for_each(|(channel_kind, channel)| {
                while let Some((tick, single_data)) = channel.receiver.read_message() {
                    trace!(?channel_kind, ?tick, ?single_data, "received message");
                    let mut reader = Reader::from(single_data);
                    // TODO: get const type ids
                    if channel_kind == &ChannelKind::of::<PingChannel>() {
                        let ping = Ping::from_bytes(&mut reader)?;
                        // prepare a pong in response (but do not send yet, because we need
                        // to set the correct send time)
                        self.ping_manager
                            .buffer_pending_pong(&ping, time_manager.current_time());
                        trace!("buffer pong");
                    } else if channel_kind == &ChannelKind::of::<PongChannel>() {
                        let pong = Pong::from_bytes(&mut reader)?;
                        // process the pong
                        self.ping_manager
                            .process_pong(&pong, time_manager.current_time());
                    } else if channel_kind == &ChannelKind::of::<EntityActionsChannel>() {
                        let actions = EntityActionsMessage::from_bytes(&mut reader)?;
                        trace!(?tick, ?actions, "received replication actions message");
                        // buffer the replication message
                        self.replication_receiver.recv_actions(actions, tick);
                    } else if channel_kind == &ChannelKind::of::<EntityUpdatesChannel>() {
                        let updates = EntityUpdatesMessage::from_bytes(&mut reader)?;
                        trace!(?tick, ?updates, "received replication updates message");
                        // buffer the replication message
                        self.replication_receiver.recv_updates(updates, tick);
                    } else {
                        // TODO: THIS IS DUPLICATED FROM THE `receive_message` FUNCTION BUT THERE ARE BORROW CHECKER
                        //  BECAUSE SPLIT BORROWS ARE NOT WELL HANDLED!

                        // TODO: we only get RawData here, does that mean we're deserializing multiple times?
                        //  instead just read the bytes for the target!!
                        let ClientMessage { message, target } =
                            ClientMessage::from_bytes(&mut reader)?;
                        // dbg!(message.as_ref());

                        let mut reader = Reader::from(message);
                        let net_id = NetId::from_bytes(&mut reader)?;
                        // we are also sending target and channel kind so the message can be
                        // rebroadcasted to other clients after we have converted the entities from the
                        // client World to the server World
                        // TODO: but do we have data to convert the entities from the client to the server?
                        //  I don't think so... maybe the sender should map_entities themselves?
                        //  or it matters for input messages?
                        // TODO: avoid clone with Arc<[u8]>?
                        let bytes = reader.consume();
                        self.received_messages.push((bytes, target, *channel_kind));
                    }
                }
                Ok::<(), SerializationError>(())
            })?;

        // Check if we have any replication messages we can apply to the World (and emit events)
        self.replication_receiver.apply_world(
            world,
            Some(self.client_id),
            component_registry,
            tick_manager.tick(),
            &mut self.events,
        );

        // TODO: do i really need this? I could just create events in this function directly?
        //  why do i need to make events a field of the connection?
        //  is it because of push_connection?
        Ok(core::mem::replace(&mut self.events, ConnectionEvents::new()))
    }

    /// Receive bytes for a single message.
    ///
    /// Adds them to an internal buffer, so that we can decode them into the correct type.
    pub(crate) fn receive_message(
        &mut self,
        mut reader: Reader,
        channel_kind: ChannelKind,
        message_registry: &MessageRegistry,
    ) -> Result<(), SerializationError> {
        // TODO: we only get RawData here, does that mean we're deserializing multiple times?
        //  instead just read the bytes for the target!!
        let ClientMessage { message, target } = ClientMessage::from_bytes(&mut reader)?;

        let mut reader = Reader::from(message);
        let net_id = NetId::from_bytes(&mut reader)?;
        // we are also sending target and channel kind so the message can be
        // rebroadcasted to other clients after we have converted the entities from the
        // client World to the server World
        // TODO: avoid clone with Arc<[u8]>?
        let bytes = reader.consume();
        self.received_messages.push((bytes, target, channel_kind));
        Ok(())
    }

    pub fn recv_packet(
        &mut self,
        packet: RecvPayload,
        tick_manager: &TickManager,
        component_registry: &ComponentRegistry,
        delta_manager: &mut DeltaManager,
    ) -> Result<(), ServerError> {
        // receive the packets, buffer them, update any sender that were waiting for their sent messages to be acked
        let tick = self.message_manager.recv_packet(packet)?;
        // notify the replication sender that some sent messages were received
        self.replication_sender
            .recv_update_acks(component_registry, delta_manager);
        trace!("Received server packet with tick: {:?}", tick);
        Ok(())
    }

    /// Helper function to prepare component insert for components for which we know the type
    pub(crate) fn prepare_typed_component_insert<C: Component>(
        &mut self,
        entity: Entity,
        group_id: ReplicationGroupId,
        component_registry: &ComponentRegistry,
        data: &C,
    ) -> Result<(), ServerError> {
        let net_id = component_registry
            .get_net_id::<C>()
            .ok_or::<ServerError>(ComponentError::NotRegistered.into())?;
        // TODO: add SendEntityMap here!
        // We store the Bytes in a hashmap, maybe more efficient to write the replication message directly?
        component_registry.serialize(data, &mut self.writer, &mut SendEntityMap::default())?;
        let raw_data = self.writer.split();
        self.replication_sender
            .prepare_component_insert(entity, group_id, raw_data);
        Ok(())
    }
}

impl ConnectionManager {
    pub(crate) fn prepare_entity_despawn(
        &mut self,
        mut entity: Entity,
        group_id: ReplicationGroupId,
        target: NetworkTarget,
    ) -> Result<(), ServerError> {
        connected_targets_mut(&mut self.connections, &target).try_for_each(|connection| {
            // trace!(
            //     ?entity,
            //     ?client_id,
            //     "Send entity despawn for tick {:?}",
            //     self.tick_manager.tick()
            // );

            // convert the entity to a network entity (possibly mapped)
            entity = connection
                .replication_receiver
                .remote_entity_map
                .to_remote(entity);

            connection
                .replication_sender
                .prepare_entity_despawn(entity, group_id);
            Ok(())
        })
    }

    pub(crate) fn prepare_component_remove(
        &mut self,
        mut entity: Entity,
        kind: ComponentNetId,
        group: &ReplicationGroup,
        target: NetworkTarget,
    ) -> Result<(), ServerError> {
        let group_id = group.group_id(Some(entity));
        debug!(?entity, ?kind, "Sending RemoveComponent");
        connected_targets_mut(&mut self.connections, &target).try_for_each(|connection| {
            entity = connection
                .replication_receiver
                .remote_entity_map
                .to_remote(entity);
            // TODO: I don't think it's actually correct to only correct the changes since that action.
            //  what if we do:
            //  - Frame 1: update is ACKED
            //  - Frame 2: update
            //  - Frame 3: action
            //  - Frame 4: send
            //  then we won't send the frame-2 update because we only collect changes since frame 3
            connection
                .replication_sender
                .prepare_component_remove(entity, group_id, kind);
            Ok(())
        })
    }

    // TODO: perf gain if we batch this? (send vec of components) (same for update/removes)
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn prepare_component_insert(
        &mut self,
        entity: Entity,
        kind: ComponentKind,
        component_data: Ptr,
        component_registry: &ComponentRegistry,
        prediction_target: Option<&NetworkTarget>,
        group_id: ReplicationGroupId,
        target: NetworkTarget,
        delta_compression: bool,
        tick: Tick,
    ) -> Result<(), ServerError> {
        // TODO: first check that the target is not empty!

        // TODO: think about this. this feels a bit clumsy
        // TODO: this might not be required anymore since we separated ShouldBePredicted from PrePredicted

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

        // same thing for PreSpawned: that component should only be replicated to prediction_target
        let mut actual_target = target;
        let should_be_predicted_kind = ComponentKind::of::<ShouldBePredicted>();
        let pre_spawned_player_object_kind = ComponentKind::of::<PreSpawned>();
        if kind == should_be_predicted_kind || kind == pre_spawned_player_object_kind {
            actual_target = prediction_target.unwrap().clone();
        }

        // even with delta-compression enabled
        // the diff can be shared for every client since we're inserting
        if delta_compression {
            // store the component value in a storage shared between all connections, so that we can compute diffs
            // Be mindful that we use the local entity for this, so that it can be shared between all connections
            // NOTE: we don't update the ack data because we only receive acks for ReplicationUpdate messages
            self.delta_manager.data.store_component_value(
                entity,
                tick,
                kind,
                component_data,
                group_id,
                component_registry,
            );
        }

        // there is no entity mapping, so we can serialize the component once for all clients
        let mut raw_data: Option<Bytes> = None;
        if !component_registry.erased_is_map_entities(kind) {
            if delta_compression {
                // SAFETY: the component_data corresponds to the kind
                unsafe {
                    component_registry.serialize_diff_from_base_value(
                        component_data,
                        &mut self.writer,
                        kind,
                        &mut SendEntityMap::default(),
                    )?;
                }
            } else {
                component_registry.erased_serialize(
                    component_data,
                    &mut self.writer,
                    kind,
                    &mut SendEntityMap::default(),
                )?;
            };
            raw_data = Some(self.writer.split());
        }
        for connection in connected_targets_mut(&mut self.connections, &actual_target) {
            // convert the entity to a network entity (in case we need to map it)
            let entity = connection
                .replication_receiver
                .remote_entity_map
                .to_remote(entity);

            // there is entity mapping, so we might need to serialize the component differently for each client
            // (although most of the time there is not mapping done on the send side)
            // It would be nice if we could check ahead of time if there is any mapping that needs to be done
            if raw_data.is_none() {
                if delta_compression {
                    // SAFETY: the component_data corresponds to the kind
                    unsafe {
                        component_registry.serialize_diff_from_base_value(
                            component_data,
                            &mut self.writer,
                            kind,
                            // we do this to avoid split-borrow errors...
                            &mut connection
                                .replication_receiver
                                .remote_entity_map
                                .local_to_remote,
                        )?;
                    }
                } else {
                    component_registry.erased_serialize(
                        component_data,
                        &mut self.writer,
                        kind,
                        // we do this to avoid split-borrow errors...
                        &mut connection
                            .replication_receiver
                            .remote_entity_map
                            .local_to_remote,
                    )?;
                };
                // write a new message for each client, because we need to do entity mapping
                raw_data = Some(self.writer.split());
            }

            // trace!(
            //     ?entity,
            //     component = ?kind,
            //     tick = ?self.tick_manager.tick(),
            //     "Inserting single component"
            // );

            // update the collect changes tick
            // replication_sender
            //     .group_channels
            //     .entry(group)
            //     .or_default()
            //     .update_collect_changes_since_this_tick(system_current_tick);
            connection.replication_sender.prepare_component_insert(
                entity,
                group_id,
                raw_data.clone().unwrap(),
            );
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn prepare_component_update(
        &mut self,
        entity: Entity,
        kind: ComponentKind,
        component: Ptr,
        registry: &ComponentRegistry,
        group_id: ReplicationGroupId,
        target: NetworkTarget,
        component_change_tick: BevyTick,
        system_current_tick: BevyTick,
        tick: Tick,
        delta_compression: bool,
    ) -> Result<(), ServerError> {
        let mut num_targets = 0;
        let mut existing_bytes: Option<Bytes> = None;
        connected_targets_mut(&mut self.connections,&target).try_for_each(|connection| {
            let send_tick = connection
                .replication_sender
                .group_channels
                .entry(group_id)
                .or_default()
                .send_tick;
            // send the update for all changes newer than the last send_tick for the group
            trace!(
                name = ?registry.name(kind),
                ?kind,
                change_tick = ?component_change_tick,
                ?send_tick,
                "prepare entity update changed check (we want the component-change-tick to be higher than send_tick)"
            );

            if send_tick.map_or(true, |tick| {
                component_change_tick.is_newer_than(tick, system_current_tick)
            }) {
                num_targets += 1;
                debug!(
                    ?entity,
                    ?tick,
                    name = ?registry.name(kind),
                    "Updating single component"
                );



                if delta_compression {
                    connection.replication_sender.prepare_delta_component_update(entity, group_id, kind, component, registry, &mut connection.writer, &mut self.delta_manager, tick, &mut connection
                        .replication_receiver.remote_entity_map)?;
                } else {
                    // we serialize once and re-use the result for all clients
                    // serialize only if there is at least one client that needs the update
                    if existing_bytes.is_none() || registry.erased_is_map_entities(kind) {
                        registry.erased_serialize(component, &mut connection.writer, kind, &mut connection.replication_receiver.remote_entity_map.local_to_remote)?;
                        // we re-serialize every time if there is entity mapping
                        existing_bytes = Some(connection.writer.split());
                    }
                    let raw_data = existing_bytes.clone().unwrap();
                    // use the network entity
                    let entity = connection
                        .replication_receiver
                        .remote_entity_map
                        .to_remote(entity);
                    connection.replication_sender.prepare_component_update(entity, group_id, raw_data);
                }
            }
            Ok::<(), ServerError>(())
        })?;

        if delta_compression && num_targets > 0 {
            // store the component value in a storage shared between all connections, so that we can compute diffs
            self.delta_manager
                .data
                .store_component_value(entity, tick, kind, component, group_id, registry);
            // register the number of clients that the component was sent to
            // (if we receive an ack from all these clients for a given tick, we can remove the component value from the storage
            //  for all the ticks that are older than the last acked tick)
            // TODO: if clients 1 and 2 send an ACK for tick 3, and client 3 sends an ack for tick 5 (but lost tick 3),
            //  we should still consider that we can delete all the data older than tick 3!
            self.delta_manager
                .acks
                .entry(group_id)
                .or_default()
                .insert(tick, num_targets);
        }

        Ok(())
    }
}

// impl EventSend for ConnectionManager {}
//
// impl InternalEventSend for ConnectionManager {
//     type Error = ServerError;
//
//     fn erased_send_event_to_target<E: Event>(
//         &mut self,
//         event: &E,
//         channel_kind: ChannelKind,
//         target: NetworkTarget,
//     ) -> Result<(), Self::Error> {
//         if self.message_registry.is_map_entities::<E>() {
//             self.buffer_map_entities_event(
//                 event,
//                 EventReplicationMode::Buffer,
//                 channel_kind,
//                 target,
//             )?;
//         } else {
//             self.message_registry.serialize_event(
//                 event,
//                 EventReplicationMode::Buffer,
//                 &mut self.writer,
//                 &mut SendEntityMap::default(),
//             )?;
//             let message_bytes = self.writer.split();
//             self.buffer_message_bytes(message_bytes, channel_kind, target)?;
//         }
//         Ok(())
//     }
//
//     fn erased_trigger_event_to_target<E: Event + Message>(
//         &mut self,
//         event: &E,
//         channel_kind: ChannelKind,
//         target: NetworkTarget,
//     ) -> Result<(), Self::Error> {
//         if self.message_registry.is_map_entities::<E>() {
//             self.buffer_map_entities_event(
//                 event,
//                 EventReplicationMode::Trigger,
//                 channel_kind,
//                 target,
//             )?;
//         } else {
//             self.message_registry.serialize_event(
//                 event,
//                 EventReplicationMode::Trigger,
//                 &mut self.writer,
//                 &mut SendEntityMap::default(),
//             )?;
//             let message_bytes = self.writer.split();
//             self.buffer_message_bytes(message_bytes, channel_kind, target)?;
//         }
//         Ok(())
//     }
// }

impl ReplicationPeer for ConnectionManager {
    type Events = ServerEvents;
    type EventContext = ClientId;
    type SetMarker = ServerMarker;
}

impl ReplicationReceive for ConnectionManager {
    fn events(&mut self) -> &mut Self::Events {
        &mut self.events
    }

    fn cleanup(&mut self, tick: Tick) {
        debug!("Running replication receive cleanup");
        for connection in self.connections.values_mut() {
            connection.replication_receiver.cleanup(tick);
        }
    }
}

impl ReplicationSend for ConnectionManager {
    type Error = ServerError;

    fn writer(&mut self) -> &mut Writer {
        &mut self.writer
    }

    fn new_connected_clients(&self) -> Vec<ClientId> {
        self.new_clients.clone()
    }

    fn cleanup(&mut self, tick: Tick) {
        debug!("Running replication send cleanup");
        for connection in self.connections.values_mut() {
            connection.replication_sender.cleanup(tick);
        }
    }
}
