//! Specify how a Server sends/receives messages with a Client
use anyhow::{Context, Result};
use bevy::ecs::component::Tick as BevyTick;
use bevy::ecs::entity::{EntityHash, MapEntities};
use bevy::prelude::{Component, Entity, Mut, Resource, World};
use bevy::ptr::Ptr;
use bevy::utils::{HashMap, HashSet};
use bytes::Bytes;
use hashbrown::hash_map::Entry;
use serde::Serialize;
use tracing::{debug, error, info, trace, trace_span, warn};

use crate::channel::builder::{EntityUpdatesChannel, PingChannel};
use bitcode::encoding::Fixed;

use crate::channel::senders::ChannelSend;
use crate::client::message::ClientMessage;
use crate::connection::id::ClientId;
use crate::inputs::native::input_buffer::InputBuffer;
use crate::packet::message_manager::MessageManager;
use crate::packet::packet::Packet;
use crate::packet::packet_manager::{Payload, PACKET_BUFFER_CAPACITY};
use crate::prelude::server::{DisconnectEvent, RoomId, RoomManager};
use crate::prelude::{
    Channel, ChannelKind, Message, Mode, PreSpawnedPlayerObject, ReplicationGroup,
    ShouldBePredicted, TargetEntity,
};
use crate::protocol::channel::ChannelRegistry;
use crate::protocol::component::{ComponentKind, ComponentNetId, ComponentRegistry};
use crate::protocol::message::{MessageRegistry, MessageType};
use crate::protocol::registry::NetId;
use crate::protocol::BitSerializable;
use crate::serialize::bitcode::reader::BufferPool;
use crate::serialize::bitcode::writer::BitcodeWriter;
use crate::serialize::reader::ReadBuffer;
use crate::serialize::writer::WriteBuffer;
use crate::serialize::RawData;
use crate::server::config::{PacketConfig, ReplicationConfig};
use crate::server::events::{ConnectEvent, ServerEvents};
use crate::server::message::ServerMessage;
use crate::server::replication::send::ReplicateCache;
use crate::shared::events::connection::ConnectionEvents;
use crate::shared::message::MessageSend;
use crate::shared::ping::manager::{PingConfig, PingManager};
use crate::shared::ping::message::{Ping, Pong, SyncMessage};
use crate::shared::replication::components::{
    Controlled, ReplicationGroupId, ReplicationTarget, ShouldBeInterpolated,
};
use crate::shared::replication::delta::{DeltaManager, Diffable};
use crate::shared::replication::network_target::NetworkTarget;
use crate::shared::replication::receive::ReplicationReceiver;
use crate::shared::replication::send::ReplicationSender;
use crate::shared::replication::{ReplicationMessage, ReplicationReceive, ReplicationSend};
use crate::shared::replication::{ReplicationMessageData, ReplicationPeer};
use crate::shared::sets::ServerMarker;
use crate::shared::tick_manager::Tick;
use crate::shared::tick_manager::TickManager;
use crate::shared::time_manager::TimeManager;

type EntityHashMap<K, V> = hashbrown::HashMap<K, V, EntityHash>;

#[derive(Resource)]
pub struct ConnectionManager {
    pub(crate) connections: HashMap<ClientId, Connection>,
    pub(crate) message_registry: MessageRegistry,
    channel_registry: ChannelRegistry,
    pub(crate) events: ServerEvents,
    pub(crate) delta_manager: DeltaManager,

    // NOTE: we put this here because we only need one per world, not one per connection
    /// Stores some values that are needed to correctly replicate the despawning of Replicated entity.
    /// (when the entity is despawned, we don't have access to its components anymore, so we cache them here)
    pub(crate) replicate_component_cache: EntityHashMap<Entity, ReplicateCache>,

    // list of clients that connected since the last time we sent replication messages
    // (we want to keep track of them because we need to replicate the entire world state to them)
    pub(crate) new_clients: Vec<ClientId>,
    pub(crate) writer: BitcodeWriter,
    pub(crate) reader_pool: BufferPool,

    // CONFIG
    replication_config: ReplicationConfig,
    packet_config: PacketConfig,
    ping_config: PingConfig,
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
            replicate_component_cache: EntityHashMap::default(),
            new_clients: vec![],
            writer: BitcodeWriter::with_capacity(PACKET_BUFFER_CAPACITY),
            reader_pool: BufferPool::new(1),
            replication_config,
            packet_config,
            ping_config,
        }
    }

    /// Return the [`Entity`] associated with the given [`ClientId`]
    pub fn client_entity(&self, client_id: ClientId) -> Result<Entity> {
        self.connection(client_id).map(|c| c.entity)
    }

    /// Return the list of connected [`ClientId`]s
    pub fn connected_clients(&self) -> impl Iterator<Item = ClientId> + '_ {
        self.connections.keys().copied()
    }

    /// Queues up a message to be sent to all clients matching the specific [`NetworkTarget`]
    pub fn send_message_to_target<C: Channel, M: Message>(
        &mut self,
        message: &M,
        target: NetworkTarget,
    ) -> Result<()> {
        self.erased_send_message_to_target(message, ChannelKind::of::<C>(), target)
    }

    /// Send a message to all clients in a room
    pub fn send_message_to_room<C: Channel, M: Message>(
        &mut self,
        message: &M,
        room_id: RoomId,
        room_manager: &RoomManager,
    ) -> Result<()> {
        let room = room_manager.get_room(room_id).context("room not found")?;
        let target = NetworkTarget::Only(room.clients.iter().copied().collect());
        self.send_message_to_target::<C, M>(message, target)
    }

    /// Queues up a message to be sent to a client
    pub fn send_message<C: Channel, M: Message>(
        &mut self,
        client_id: ClientId,
        message: &M,
    ) -> Result<()> {
        self.send_message_to_target::<C, M>(message, NetworkTarget::Only(vec![client_id]))
    }

    /// Update the priority of a `ReplicationGroup` that is replicated to a given client
    pub fn update_priority(
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
        self.connection_mut(client_id)?
            .replication_sender
            .update_base_priority(replication_group_id, priority);
        Ok(())
    }

    /// Find the list of clients that should receive the replication message
    pub(crate) fn apply_replication(
        &mut self,
        target: NetworkTarget,
    ) -> Box<dyn Iterator<Item = ClientId>> {
        let connected_clients = self.connections.keys().copied().collect::<Vec<_>>();
        match target {
            NetworkTarget::All => {
                // TODO: maybe only send stuff when the client is time-synced ?
                Box::new(connected_clients.into_iter())
            }
            NetworkTarget::AllExceptSingle(client_id) => Box::new(
                connected_clients
                    .into_iter()
                    .filter(move |id| *id != client_id),
            ),
            NetworkTarget::AllExcept(client_ids) => {
                let client_ids: HashSet<ClientId> = HashSet::from_iter(client_ids);
                Box::new(
                    connected_clients
                        .into_iter()
                        .filter(move |id| !client_ids.contains(id)),
                )
            }
            NetworkTarget::Single(client_id) => {
                if self.connections.contains_key(&client_id) {
                    Box::new(std::iter::once(client_id))
                } else {
                    Box::new(std::iter::empty())
                }
            }
            NetworkTarget::Only(client_ids) => Box::new(
                connected_clients
                    .into_iter()
                    .filter(move |id| client_ids.contains(id)),
            ),
            NetworkTarget::None => Box::new(std::iter::empty()),
        }
    }

    pub(crate) fn connection(&self, client_id: ClientId) -> Result<&Connection> {
        self.connections
            .get(&client_id)
            .context("client id not found")
    }

    pub(crate) fn connection_mut(&mut self, client_id: ClientId) -> Result<&mut Connection> {
        self.connections
            .get_mut(&client_id)
            .context("client id not found")
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
        if let Entry::Vacant(e) = self.connections.entry(client_id) {
            #[cfg(feature = "metrics")]
            metrics::gauge!("connected_clients").increment(1.0);

            info!("New connection from id: {}", client_id);
            let connection = Connection::new(
                client_id,
                client_entity,
                &self.channel_registry,
                self.replication_config.clone(),
                self.packet_config.clone(),
                self.ping_config.clone(),
            );
            self.events.add_connect_event(ConnectEvent {
                client_id,
                entity: client_entity,
            });
            self.new_clients.push(client_id);
            e.insert(connection);
        } else {
            info!("Client {} was already in the connections list", client_id);
        }
    }

    /// Remove the connection associated with the given [`ClientId`],
    /// and returns the [`Entity`] associated with the client
    pub(crate) fn remove(&mut self, client_id: ClientId) -> Entity {
        #[cfg(feature = "metrics")]
        metrics::gauge!("connected_clients").decrement(1.0);

        info!("Client {} disconnected", client_id);
        let entity = self
            .client_entity(client_id)
            .expect("client entity not found");
        self.events
            .add_disconnect_event(DisconnectEvent { client_id, entity });
        self.connections.remove(&client_id);
        entity
    }

    pub(crate) fn buffer_message(
        &mut self,
        message: RawData,
        channel: ChannelKind,
        target: NetworkTarget,
    ) -> Result<()> {
        self.connections
            .iter_mut()
            .filter(|(id, _)| target.targets(id))
            // TODO: is it worth it to use Arc<Vec<u8>> or Bytes to have a free clone?
            //  at some point the bytes will have to be copied into the final message, so maybe do it now?
            .try_for_each(|(_, c)| c.buffer_message(message.clone(), channel))
    }

    pub(crate) fn erased_send_message_to_target<M: Message>(
        &mut self,
        message: &M,
        channel_kind: ChannelKind,
        target: NetworkTarget,
    ) -> Result<()> {
        let message_bytes = self
            .message_registry
            .serialize(message, &mut self.writer)
            .context("could not serialize message")?;
        self.buffer_message(message_bytes, channel_kind, target)
    }

    /// Buffer all the replication messages to send.
    /// Keep track of the bevy Change Tick: when a message is acked, we know that we only have to send
    /// the updates since that Change Tick
    pub(crate) fn buffer_replication_messages(
        &mut self,
        tick: Tick,
        bevy_tick: BevyTick,
    ) -> Result<()> {
        let _span = trace_span!("buffer_replication_messages").entered();
        self.connections
            .values_mut()
            .try_for_each(move |c| c.buffer_replication_messages(tick, bevy_tick))
    }

    pub(crate) fn receive(
        &mut self,
        world: &mut World,
        time_manager: &TimeManager,
        tick_manager: &TickManager,
    ) -> Result<()> {
        let mut messages_to_rebroadcast = vec![];
        // TODO: do this in parallel
        self.connections
            .iter_mut()
            .for_each(|(client_id, connection)| {
                let _span = trace_span!("receive", ?client_id).entered();
                world.resource_scope(|world, component_registry: Mut<ComponentRegistry>| {
                    // receive events on the connection
                    let events = connection.receive(
                        world,
                        component_registry.as_ref(),
                        time_manager,
                        tick_manager,
                    );
                    // move the events from the connection to the connection manager
                    self.events.push_events(*client_id, events);
                });

                // rebroadcast messages
                messages_to_rebroadcast
                    .extend(std::mem::take(&mut connection.messages_to_rebroadcast));
            });
        for (message, target, channel_kind) in messages_to_rebroadcast {
            self.buffer_message(message, channel_kind, target)?;
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
        data: &C,
        bevy_tick: BevyTick,
    ) -> Result<()> {
        let net_id = component_registry
            .get_net_id::<C>()
            .context(format!("{} is not registered", std::any::type_name::<C>()))?;
        let raw_data = component_registry.serialize(data, &mut self.writer)?;
        self.connection_mut(client_id)?
            .replication_sender
            .prepare_component_insert(entity, group_id, raw_data, bevy_tick);
        Ok(())
    }
}

/// Wrapper that handles the connection between the server and a client
pub struct Connection {
    client_id: ClientId,
    /// We create one entity per connected client, so that users
    /// can store metadata about the client using the ECS
    entity: Entity,
    pub(crate) message_manager: MessageManager,
    pub(crate) replication_sender: ReplicationSender,
    pub(crate) replication_receiver: ReplicationReceiver,
    pub(crate) events: ConnectionEvents,
    pub(crate) ping_manager: PingManager,

    // TODO: maybe don't do any replication until connection is synced?
    /// Used to transfer raw bytes to a system that can convert the bytes to the actual type
    pub(crate) received_messages: HashMap<NetId, Vec<(Bytes, NetworkTarget, ChannelKind)>>,
    pub(crate) received_input_messages: HashMap<NetId, Vec<(Bytes, NetworkTarget, ChannelKind)>>,
    #[cfg(feature = "leafwing")]
    pub(crate) received_leafwing_input_messages:
        HashMap<NetId, Vec<(Bytes, NetworkTarget, ChannelKind)>>,
    writer: BitcodeWriter,
    pub(crate) reader_pool: BufferPool,
    // messages that we have received that need to be rebroadcasted to other clients
    pub(crate) messages_to_rebroadcast: Vec<(RawData, NetworkTarget, ChannelKind)>,
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
            replication_config.send_updates_since_last_ack,
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
            received_messages: HashMap::default(),
            received_input_messages: HashMap::default(),
            #[cfg(feature = "leafwing")]
            received_leafwing_input_messages: HashMap::default(),
            writer: BitcodeWriter::with_capacity(PACKET_BUFFER_CAPACITY),
            // TODO: it looks like we don't really need the pool this case, we can just keep re-using the same buffer
            reader_pool: BufferPool::new(1),
            messages_to_rebroadcast: vec![],
        }
    }

    pub(crate) fn update(
        &mut self,
        world_tick: BevyTick,
        time_manager: &TimeManager,
        tick_manager: &TickManager,
    ) {
        self.message_manager
            .update(time_manager, &self.ping_manager, tick_manager);
        self.replication_sender.update(world_tick);
        self.ping_manager.update(time_manager);
    }

    pub(crate) fn buffer_message(&mut self, message: Vec<u8>, channel: ChannelKind) -> Result<()> {
        // TODO: i know channel names never change so i should be able to get them as static
        // TODO: just have a channel registry enum as well?
        let channel_name = self
            .message_manager
            .channel_registry
            .name(&channel)
            .unwrap_or("unknown")
            .to_string();
        let message = ServerMessage::Message(message);
        self.writer.start_write();
        message.encode(&mut self.writer)?;
        // TODO: doesn't this serialize the bytes twice?
        let message_bytes = self.writer.finish_write().to_vec();
        // message.emit_send_logs(&channel_name);
        self.message_manager.buffer_send(message_bytes, channel)?;
        Ok(())
    }

    pub(crate) fn buffer_replication_messages(
        &mut self,
        tick: Tick,
        bevy_tick: BevyTick,
    ) -> Result<()> {
        self.replication_sender
            .finalize(tick)
            .into_iter()
            .try_for_each(|(channel, group_id, message_data, priority)| {
                let should_track_ack = matches!(message_data, ReplicationMessageData::Updates(_));
                let channel_name = self
                    .message_manager
                    .channel_registry
                    .name(&channel)
                    .unwrap_or("unknown")
                    .to_string();
                let message = ClientMessage::Replication(ReplicationMessage {
                    group_id,
                    data: message_data,
                });
                self.writer.start_write();
                message.encode(&mut self.writer)?;
                // TODO: doesn't this serialize the bytes twice?
                let message_bytes = self.writer.finish_write().to_vec();
                // message.emit_send_logs(&channel_name);
                let message_id = self
                    .message_manager
                    .buffer_send_with_priority(message_bytes, channel, priority)?
                    .expect("The replication channels should always return a message_id");

                // keep track of the group associated with the message, so we can handle receiving an ACK for that message_id later
                if should_track_ack {
                    self.replication_sender
                        .buffer_replication_update_message(group_id, message_id, bevy_tick, tick);
                }
                Ok(())
            })
    }

    fn send_ping(&mut self, ping: Ping) -> Result<()> {
        trace!("Sending ping {:?}", ping);
        self.writer.start_write();
        ServerMessage::Ping(ping).encode(&mut self.writer)?;
        let message_bytes = self.writer.finish_write().to_vec();
        self.message_manager
            .buffer_send(message_bytes, ChannelKind::of::<PingChannel>())?;
        Ok(())
    }

    fn send_pong(&mut self, pong: Pong) -> Result<()> {
        trace!("Sending pong {:?}", pong);
        self.writer.start_write();
        ServerMessage::Pong(pong).encode(&mut self.writer)?;
        let message_bytes = self.writer.finish_write().to_vec();
        self.message_manager
            .buffer_send(message_bytes, ChannelKind::of::<PingChannel>())?;
        Ok(())
    }

    /// Send packets that are ready to be sent
    pub fn send_packets(
        &mut self,
        time_manager: &TimeManager,
        tick_manager: &TickManager,
    ) -> Result<Vec<Payload>> {
        // update the ping manager with the actual send time
        // TODO: issues here: we would like to send the ping/pong messages immediately, otherwise the recorded current time is incorrect
        //   - can give infinity priority to this channel?
        //   - can write directly to io otherwise?

        // no need to check if `time_manager.is_server_ready_to_send()` since we only send packets when we are ready to send
        if time_manager.is_server_ready_to_send() {
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
                    Ok::<(), anyhow::Error>(())
                })?;
        }
        let payloads = self.message_manager.send_packets(tick_manager.tick());

        // update the replication sender about which messages were actually sent, and accumulate priority
        self.replication_sender.recv_send_notification();
        payloads
    }

    pub fn receive(
        &mut self,
        world: &mut World,
        component_registry: &ComponentRegistry,
        time_manager: &TimeManager,
        tick_manager: &TickManager,
    ) -> ConnectionEvents {
        let _span = trace_span!("receive").entered();
        let message_registry = world.resource::<MessageRegistry>();
        for (channel_kind, messages) in self.message_manager.read_messages() {
            let channel_name = self
                .message_manager
                .channel_registry
                .name(&channel_kind)
                .unwrap_or("unknown");
            let _span_channel = trace_span!("channel", channel = channel_name).entered();

            if !messages.is_empty() {
                trace!(?channel_name, ?messages, "Received messages");
                for (tick, single_data) in messages.into_iter() {
                    trace!(?tick, ?single_data, "received message");
                    // TODO: in this case, it looks like we might not need the pool?
                    //  we can just have a single buffer, and keep re-using that buffer
                    let mut reader = self.reader_pool.start_read(single_data.as_ref());
                    // TODO: maybe just decode a single bit to know if it's message vs replication?
                    let message = ClientMessage::decode(&mut reader)
                        .expect("Could not decode server message");
                    self.reader_pool.attach(reader);

                    match message {
                        ClientMessage::Message(message, target) => {
                            let mut reader = self.reader_pool.start_read(message.as_slice());
                            let net_id = reader
                                .decode::<NetId>(Fixed)
                                .expect("could not decode MessageKind");
                            self.reader_pool.attach(reader);

                            // we are also sending target and channel kind so the message can be
                            // rebroadcasted to other clients after we have converted the entities from the
                            // client World to the server World
                            // TODO: but do we have data to convert the entities from the client to the server?
                            //  I don't think so... maybe the sender should map_entities themselves?
                            //  or it matters for input messages?
                            // TODO: avoid clone with Arc<[u8]>?
                            let data = (message.clone().into(), target.clone(), channel_kind);

                            match message_registry.message_type(net_id) {
                                #[cfg(feature = "leafwing")]
                                MessageType::LeafwingInput => self
                                    .received_leafwing_input_messages
                                    .entry(net_id)
                                    .or_default()
                                    .push(data),
                                MessageType::NativeInput => {
                                    self.received_input_messages
                                        .entry(net_id)
                                        .or_default()
                                        .push(data);
                                }
                                MessageType::Normal => {
                                    self.received_messages.entry(net_id).or_default().push(data);
                                }
                            }
                        }
                        ClientMessage::Replication(replication) => {
                            trace!(?tick, ?replication, "received replication message");
                            // buffer the replication message
                            self.replication_receiver.recv_message(replication, tick);
                        }
                        ClientMessage::Ping(ping) => {
                            // prepare a pong in response (but do not send yet, because we need
                            // to set the correct send time)
                            self.ping_manager
                                .buffer_pending_pong(&ping, time_manager.current_time());
                            trace!("buffer pong");
                        }
                        ClientMessage::Pong(pong) => {
                            // process the pong
                            self.ping_manager
                                .process_pong(&pong, time_manager.current_time());
                        }
                    }
                }
            }
        }

        // NOTE: we run this outside `messages.is_empty()` because we might have some messages from a future tick that we can now process
        // Check if we have any replication messages we can apply to the World (and emit events)
        for (group, replication_list) in
            self.replication_receiver.read_messages(tick_manager.tick())
        {
            trace!(?group, ?replication_list, "read replication messages");
            replication_list
                .into_iter()
                .for_each(|(tick, replication)| {
                    // TODO: we could include the server tick when this replication_message was sent.
                    self.replication_receiver.apply_world(
                        world,
                        Some(self.client_id),
                        component_registry,
                        tick,
                        replication,
                        group,
                        &mut self.events,
                    );
                });
        }

        // TODO: do i really need this? I could just create events in this function directly?
        //  why do i need to make events a field of the connection?
        //  is it because of push_connection?
        std::mem::replace(&mut self.events, ConnectionEvents::new())
    }

    pub fn recv_packet(
        &mut self,
        packet: Packet,
        tick_manager: &TickManager,
        component_registry: &ComponentRegistry,
        delta_manager: &mut DeltaManager,
    ) -> Result<()> {
        // receive the packets, buffer them, update any sender that were waiting for their sent messages to be acked
        let tick = self.message_manager.recv_packet(packet)?;
        // notify the replication sender that some sent messages were received
        self.replication_sender
            .recv_update_acks(component_registry, delta_manager);
        debug!("Received server packet with tick: {:?}", tick);
        Ok(())
    }
}

impl ConnectionManager {
    pub(crate) fn prepare_entity_despawn(
        &mut self,
        entity: Entity,
        group: &ReplicationGroup,
        target: NetworkTarget,
    ) -> Result<()> {
        let group_id = group.group_id(Some(entity));
        self.apply_replication(target).try_for_each(|client_id| {
            // trace!(
            //     ?entity,
            //     ?client_id,
            //     "Send entity despawn for tick {:?}",
            //     self.tick_manager.tick()
            // );
            self.connection_mut(client_id)?
                .replication_sender
                .prepare_entity_despawn(entity, group_id);
            Ok(())
        })
    }

    pub(crate) fn prepare_component_remove(
        &mut self,
        entity: Entity,
        kind: ComponentNetId,
        group: &ReplicationGroup,
        target: NetworkTarget,
    ) -> Result<()> {
        let group_id = group.group_id(Some(entity));
        debug!(?entity, ?kind, "Sending RemoveComponent");
        self.apply_replication(target).try_for_each(|client_id| {
            // TODO: I don't think it's actually correct to only correct the changes since that action.
            //  what if we do:
            //  - Frame 1: update is ACKED
            //  - Frame 2: update
            //  - Frame 3: action
            //  - Frame 4: send
            //  then we won't send the frame-2 update because we only collect changes since frame 3
            self.connection_mut(client_id)?
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
        replication_target: &ReplicationTarget,
        prediction_target: Option<&NetworkTarget>,
        group: &ReplicationGroup,
        target: NetworkTarget,
        delta_compression: bool,
        tick: Tick,
        bevy_tick: BevyTick,
    ) -> Result<()> {
        // TODO: first check that the target is not empty!
        let group_id = group.group_id(Some(entity));

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

        // same thing for PreSpawnedPlayerObject: that component should only be replicated to prediction_target
        let mut actual_target = target;
        let should_be_predicted_kind = ComponentKind::of::<ShouldBePredicted>();
        let pre_spawned_player_object_kind = ComponentKind::of::<PreSpawnedPlayerObject>();
        if kind == should_be_predicted_kind || kind == pre_spawned_player_object_kind {
            actual_target = prediction_target.unwrap().clone();
        }

        // even with delta-compression enabled
        // the diff can be shared for every client since we're inserting
        let writer = &mut self.writer;
        let raw_data = if delta_compression {
            // store the component value in a storage shared between all connections, so that we can compute diffs
            // NOTE: we don't update the ack data because we only receive acks for ReplicationUpdate messages
            self.delta_manager.data.store_component_value(
                entity,
                tick,
                kind,
                component_data,
                group_id,
                component_registry,
            );
            // SAFETY: the component_data corresponds to the kind
            unsafe {
                component_registry
                    .serialize_diff_from_base_value(component_data, writer, kind)
                    .expect("could not serialize delta")
            }
        } else {
            component_registry
                .erased_serialize(component_data, writer, kind)
                .expect("could not serialize component")
        };
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
                self.connection_mut(client_id)?
                    .replication_sender
                    // TODO: avoid the clone by using Arc<u8>?
                    .prepare_component_insert(entity, group_id, raw_data.clone(), bevy_tick);
                Ok(())
            })
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn prepare_component_update(
        &mut self,
        entity: Entity,
        kind: ComponentKind,
        component: Ptr,
        registry: &ComponentRegistry,
        group: &ReplicationGroup,
        target: NetworkTarget,
        component_change_tick: BevyTick,
        system_current_tick: BevyTick,
        tick: Tick,
        delta_compression: bool,
    ) -> Result<()> {
        trace!(
            ?kind,
            ?entity,
            ?component_change_tick,
            ?system_current_tick,
            "Prepare entity update"
        );

        let group_id = group.group_id(Some(entity));
        let mut raw_data: RawData = vec![];
        if !delta_compression {
            // we serialize once and re-use the result for all clients
            raw_data = registry.erased_serialize(component, &mut self.writer, kind)?;
        }
        let mut num_targets = 0;
        self.apply_replication(target).try_for_each(|client_id| {
            // TODO: should we have additional state tracking so that we know we are in the process of sending this entity to clients?
            let replication_sender = &mut self.connections.get_mut(&client_id).context("cannot find connection")?.replication_sender;
            let send_tick = replication_sender
                .group_channels
                .entry(group_id)
                .or_default()
                .send_tick;
            // send the update for all changes newer than the last send_tick for the group
            debug!(
                ?kind,
                change_tick = ?component_change_tick,
                ?send_tick,
                "prepare entity update changed check (we want the component-change-tick to be higher than send_tick)"
            );

            if send_tick.map_or(true, |tick| {
                component_change_tick.is_newer_than(tick, system_current_tick)
            }) {
                num_targets += 1;
                trace!(
                    change_tick = ?component_change_tick,
                    ?send_tick,
                    current_tick = ?system_current_tick,
                    "prepare entity update changed check"
                );
                // trace!(
                //     ?entity,
                //     component = ?kind,
                //     tick = ?self.tick_manager.tick(),
                //     "Updating single component"
                // );
                if !delta_compression {
                    // TODO: avoid component clone with Arc<[u8]>
                    replication_sender.prepare_component_update(entity, group_id, raw_data.clone());
                } else {
                    replication_sender.prepare_delta_component_update(entity, group_id, kind, component, registry, &mut self.writer, &mut self.delta_manager, tick);
                }
            }
            Ok::<(), anyhow::Error>(())
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

impl MessageSend for ConnectionManager {
    fn send_message_to_target<C: Channel, M: Message>(
        &mut self,
        message: &M,
        target: NetworkTarget,
    ) -> Result<()> {
        self.send_message_to_target::<C, M>(message, target)
    }

    fn erased_send_message_to_target<M: Message>(
        &mut self,
        message: &M,
        channel_kind: ChannelKind,
        target: NetworkTarget,
    ) -> Result<()> {
        self.erased_send_message_to_target(message, channel_kind, target)
    }
}

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
    type ReplicateCache = EntityHashMap<Entity, ReplicateCache>;

    fn writer(&mut self) -> &mut BitcodeWriter {
        &mut self.writer
    }

    fn new_connected_clients(&self) -> Vec<ClientId> {
        self.new_clients.clone()
    }

    fn replication_cache(&mut self) -> &mut Self::ReplicateCache {
        &mut self.replicate_component_cache
    }

    /// Buffer the replication messages
    fn buffer_replication_messages(&mut self, tick: Tick, bevy_tick: BevyTick) -> Result<()> {
        self.buffer_replication_messages(tick, bevy_tick)
    }

    fn cleanup(&mut self, tick: Tick) {
        debug!("Running replication send cleanup");
        for connection in self.connections.values_mut() {
            connection.replication_sender.cleanup(tick);
        }
    }
}
