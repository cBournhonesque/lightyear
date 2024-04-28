//! Specify how a Server sends/receives messages with a Client
use anyhow::{Context, Result};
use bevy::ecs::component::Tick as BevyTick;
use bevy::ecs::entity::{EntityHash, MapEntities};
use bevy::prelude::{Entity, Mut, Resource, World};
use bevy::utils::{HashMap, HashSet};
use bitcode::__private::Fixed;
use bytes::Bytes;
use hashbrown::hash_map::Entry;
use serde::Serialize;
use tracing::{debug, error, info, trace, trace_span, warn};

use crate::_internal::{
    BitSerializable, EntityUpdatesChannel, FromType, InputMessageKind, MessageKind, PingChannel,
    ReplicationSend, ServerMarker, ShouldBeInterpolated, WriteBuffer, WriteWordBuffer,
};
use crate::channel::senders::ChannelSend;
use crate::client::message::ClientMessage;
use crate::connection::id::ClientId;
use crate::inputs::native::input_buffer::InputBuffer;
use crate::packet::message_manager::MessageManager;
use crate::packet::packet::Packet;
use crate::packet::packet_manager::{Payload, PACKET_BUFFER_CAPACITY};
use crate::prelude::{
    Channel, ChannelKind, Message, Mode, PreSpawnedPlayerObject, ShouldBePredicted,
};
use crate::protocol::channel::ChannelRegistry;
use crate::protocol::component::{ComponentNetId, ComponentRegistry};
use crate::protocol::message::{MessageRegistry, MessageType};
use crate::protocol::registry::NetId;

use crate::serialize::reader::ReadBuffer;
use crate::serialize::wordbuffer::reader::BufferPool;
use crate::serialize::RawData;
use crate::server::config::PacketConfig;
use crate::server::events::ServerEvents;
use crate::server::message::ServerMessage;
use crate::shared::events::connection::ConnectionEvents;
use crate::shared::ping::manager::{PingConfig, PingManager};
use crate::shared::ping::message::{Ping, Pong, SyncMessage};
use crate::shared::replication::components::{NetworkTarget, Replicate, ReplicationGroupId};
use crate::shared::replication::receive::ReplicationReceiver;
use crate::shared::replication::send::ReplicationSender;
use crate::shared::replication::ReplicationMessage;
use crate::shared::replication::ReplicationMessageData;
use crate::shared::tick_manager::Tick;
use crate::shared::tick_manager::TickManager;
use crate::shared::time_manager::TimeManager;

type EntityHashMap<K, V> = hashbrown::HashMap<K, V, EntityHash>;

#[derive(Resource)]
pub struct ConnectionManager {
    pub(crate) connections: HashMap<ClientId, Connection>,
    pub(crate) component_registry: ComponentRegistry,
    pub(crate) message_registry: MessageRegistry,
    channel_registry: ChannelRegistry,
    pub(crate) events: ServerEvents,

    // NOTE: we put this here because we only need one per world, not one per connection
    /// Stores the last `Replicate` component for each replicated entity owned by the current world (the world that sends replication updates)
    /// Needed to know the value of the Replicate component after the entity gets despawned, to know how we replicate the EntityDespawn
    replicate_component_cache: EntityHashMap<Entity, Replicate>,

    // list of clients that connected since the last time we sent replication messages
    // (we want to keep track of them because we need to replicate the entire world state to them)
    pub(crate) new_clients: Vec<ClientId>,
    writer: WriteWordBuffer,
    pub(crate) reader_pool: BufferPool,
    packet_config: PacketConfig,
    ping_config: PingConfig,
}

impl ConnectionManager {
    pub(crate) fn new(
        component_registry: ComponentRegistry,
        message_registry: MessageRegistry,
        channel_registry: ChannelRegistry,
        packet_config: PacketConfig,
        ping_config: PingConfig,
    ) -> Self {
        Self {
            connections: HashMap::default(),
            component_registry,
            message_registry,
            channel_registry,
            events: ServerEvents::new(),
            replicate_component_cache: EntityHashMap::default(),
            new_clients: vec![],
            writer: WriteWordBuffer::with_capacity(PACKET_BUFFER_CAPACITY),
            reader_pool: BufferPool::new(1),
            packet_config,
            ping_config,
        }
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

    pub(crate) fn update(&mut self, time_manager: &TimeManager, tick_manager: &TickManager) {
        self.connections.values_mut().for_each(|connection| {
            connection.update(time_manager, tick_manager);
        });
    }

    /// Add a new [`Connection`] to the list of connections with the given [`ClientId`]
    pub(crate) fn add(&mut self, client_id: ClientId) {
        if let Entry::Vacant(e) = self.connections.entry(client_id) {
            #[cfg(feature = "metrics")]
            metrics::gauge!("connected_clients").increment(1.0);

            info!("New connection from id: {}", client_id);
            let connection = Connection::new(
                &self.channel_registry,
                self.packet_config.clone(),
                self.ping_config.clone(),
            );
            // self.events.push_connection(client_id);
            self.new_clients.push(client_id);
            e.insert(connection);
        } else {
            info!("Client {} was already in the connections list", client_id);
        }
    }

    pub(crate) fn remove(&mut self, client_id: ClientId) {
        #[cfg(feature = "metrics")]
        metrics::gauge!("connected_clients").decrement(1.0);

        info!("Client {} disconnected", client_id);
        self.events.push_disconnection(client_id);
        self.connections.remove(&client_id);
    }

    pub(crate) fn buffer_message(
        &mut self,
        message: RawData,
        channel: ChannelKind,
        target: NetworkTarget,
    ) -> Result<()> {
        self.connections
            .iter_mut()
            .filter(|(id, _)| target.should_send_to(id))
            // TODO: is it worth it to use Arc<Vec<u8>> or Bytes to have a free clone?
            //  at some point the bytes will have to be copied into the final message, so maybe do it now?
            .try_for_each(|(_, c)| c.buffer_message(message.clone(), channel))
    }

    /// Queues up a message to be sent to all clients matching the specific [`NetworkTarget`]
    pub fn send_message_to_target<C: Channel, M: Message>(
        &mut self,
        message: M,
        target: NetworkTarget,
    ) -> Result<()> {
        self.writer.start_write();
        self.message_registry
            .serialize(&message, &mut self.writer)
            .context("could not serialize message")?;
        let message_bytes = self.writer.finish_write().to_vec();
        self.buffer_message(message_bytes, ChannelKind::of::<C>(), target)
    }

    /// Queues up a message to be sent to a client
    pub fn send_message<C: Channel, M: Message>(
        &mut self,
        client_id: ClientId,
        message: M,
    ) -> Result<()> {
        self.send_message_to_target::<C, M>(message, NetworkTarget::Only(vec![client_id]))
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
                world.resource_scope(|world, mut component_registry: Mut<ComponentRegistry>| {
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

/// Wrapper that handles the connection between the server and a client
pub struct Connection {
    pub message_manager: MessageManager,
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
    writer: WriteWordBuffer,
    pub(crate) reader_pool: BufferPool,
    // messages that we have received that need to be rebroadcasted to other clients
    pub(crate) messages_to_rebroadcast: Vec<(RawData, NetworkTarget, ChannelKind)>,
}

impl Connection {
    pub(crate) fn new(
        channel_registry: &ChannelRegistry,
        packet_config: PacketConfig,
        ping_config: PingConfig,
    ) -> Self {
        // create the message manager and the channels
        let mut message_manager = MessageManager::new(channel_registry, packet_config.into());
        // get the acks-tracker for entity updates
        let update_acks_tracker = message_manager
            .channels
            .get_mut(&ChannelKind::of::<EntityUpdatesChannel>())
            .unwrap()
            .sender
            .subscribe_acks();
        // get a channel to get notified when a replication update message gets actually send (to update priority)
        let replication_update_send_receiver =
            message_manager.get_replication_update_send_receiver();
        let replication_sender =
            ReplicationSender::new(update_acks_tracker, replication_update_send_receiver);
        let replication_receiver = ReplicationReceiver::new();
        Self {
            message_manager,
            replication_sender,
            replication_receiver,
            ping_manager: PingManager::new(ping_config),
            events: ConnectionEvents::default(),
            received_messages: HashMap::default(),
            received_input_messages: HashMap::default(),
            #[cfg(feature = "leafwing")]
            received_leafwing_input_messages: HashMap::default(),
            writer: WriteWordBuffer::with_capacity(PACKET_BUFFER_CAPACITY),
            // TODO: it looks like we don't really need the pool this case, we can just keep re-using the same buffer
            reader_pool: BufferPool::new(1),
            messages_to_rebroadcast: vec![],
        }
    }

    pub(crate) fn update(&mut self, time_manager: &TimeManager, tick_manager: &TickManager) {
        self.message_manager
            .update(time_manager, &self.ping_manager, tick_manager);
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
                        .updates_message_id_to_group_id
                        .insert(message_id, (group_id, bevy_tick));
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
                        ClientMessage::Message(mut message, target) => {
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

                            if target != NetworkTarget::None {
                                self.messages_to_rebroadcast
                                    .push((message, target, channel_kind));
                            }
                        }
                        ClientMessage::Replication(replication) => {
                            // buffer the replication message
                            self.replication_receiver.recv_message(replication, tick);
                        }
                        ClientMessage::Ping(ping) => {
                            // prepare a pong in response (but do not send yet, because we need
                            // to set the correct send time)
                            self.ping_manager.buffer_pending_pong(&ping, time_manager);
                            trace!("buffer pong");
                        }
                        ClientMessage::Pong(pong) => {
                            // process the pong
                            self.ping_manager.process_pong(&pong, time_manager);
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

    pub fn recv_packet(&mut self, packet: Packet, tick_manager: &TickManager) -> Result<()> {
        // receive the packets, buffer them, update any sender that were waiting for their sent messages to be acked
        let tick = self.message_manager.recv_packet(packet)?;
        // notify the replication sender that some sent messages were received
        self.replication_sender.recv_update_acks();
        debug!("Received server packet with tick: {:?}", tick);
        Ok(())
    }
}

impl ReplicationSend for ConnectionManager {
    type Events = ServerEvents;
    type EventContext = ClientId;
    type SetMarker = ServerMarker;

    fn events(&mut self) -> &mut Self::Events {
        &mut self.events
    }

    fn writer(&mut self) -> &mut WriteWordBuffer {
        &mut self.writer
    }

    fn component_registry(&self) -> &ComponentRegistry {
        &self.component_registry
    }

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
        replicate: &Replicate,
        target: NetworkTarget,
        system_current_tick: BevyTick,
    ) -> Result<()> {
        trace!(?entity, "Prepare entity spawn to client");
        let group_id = replicate.replication_group.group_id(Some(entity));
        // TODO: should we have additional state tracking so that we know we are in the process of sending this entity to clients?
        let should_be_predicted_kind = self
            .component_registry()
            .get_net_id::<ShouldBePredicted>()
            .context("ShouldBePredicted is not registered")?;
        let should_be_interpolated_kind = self
            .component_registry()
            .get_net_id::<ShouldBeInterpolated>()
            .context("ShouldBeInterpolated is not registered")?;
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
                    should_be_predicted_kind,
                    // ShouldBePredicted is a ZST
                    vec![],
                );
            }
            if replicate.interpolation_target.should_send_to(&client_id) {
                replication_sender.prepare_component_insert(
                    entity,
                    group_id,
                    should_be_interpolated_kind,
                    // ShouldBeInterpolated is a ZST
                    vec![],
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
        replicate: &Replicate,
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
        kind: ComponentNetId,
        component: RawData,
        replicate: &Replicate,
        target: NetworkTarget,
        system_current_tick: BevyTick,
    ) -> Result<()> {
        let group_id = replicate.replication_group.group_id(Some(entity));

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
        let should_be_predicted_kind = self
            .component_registry()
            .get_net_id::<ShouldBePredicted>()
            .context("ShouldBePredicted is not registered")?;
        let pre_spawned_player_object_kind = self
            .component_registry()
            .get_net_id::<PreSpawnedPlayerObject>()
            .context("PreSpawnedPlayerObject is not registered")?;
        if kind == should_be_predicted_kind || kind == pre_spawned_player_object_kind {
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
                replication_sender.prepare_component_insert(
                    entity,
                    group_id,
                    kind,
                    component.clone(),
                );
                Ok(())
            })
    }

    fn prepare_component_remove(
        &mut self,
        entity: Entity,
        kind: ComponentNetId,
        replicate: &Replicate,
        target: NetworkTarget,
        system_current_tick: BevyTick,
    ) -> Result<()> {
        let group_id = replicate.replication_group.group_id(Some(entity));
        debug!(?entity, ?kind, "Sending RemoveComponent");
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
            replication_sender.prepare_component_remove(entity, group_id, kind);
            Ok(())
        })
    }

    fn prepare_component_update(
        &mut self,
        entity: Entity,
        kind: ComponentNetId,
        component: RawData,
        replicate: &Replicate,
        target: NetworkTarget,
        component_change_tick: BevyTick,
        system_current_tick: BevyTick,
    ) -> Result<()> {
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
                // TODO: avoid component clone with Arc<[u8]>
                replication_sender.prepare_entity_update(entity, group_id, kind, component.clone());
            }
            Ok(())
        })
    }

    /// Buffer the replication messages
    fn buffer_replication_messages(&mut self, tick: Tick, bevy_tick: BevyTick) -> Result<()> {
        self.buffer_replication_messages(tick, bevy_tick)
    }

    fn get_mut_replicate_component_cache(
        &mut self,
    ) -> &mut bevy::ecs::entity::EntityHashMap<Replicate> {
        &mut self.replicate_component_cache
    }

    fn cleanup(&mut self, tick: Tick) {
        debug!("Running replication clean");
        for connection in self.connections.values_mut() {
            // if it's been enough time since we last any action for the group, we can set the last_action_tick to None
            // (meaning that there's no need when we receive the update to check if we have already received a previous action)
            for group_channel in connection.replication_sender.group_channels.values_mut() {
                debug!("Checking group channel: {:?}", group_channel);
                if let Some(last_action_tick) = group_channel.last_action_tick {
                    if tick - last_action_tick > (i16::MAX / 2) {
                        debug!(
                    ?tick,
                    ?last_action_tick,
                    ?group_channel,
                    "Setting the last_action tick to None because there hasn't been any new actions in a while");
                        group_channel.last_action_tick = None;
                    }
                }
            }
            // if it's been enough time since we last had any update for the group, we update the latest_tick for the group
            for group_channel in connection.replication_receiver.group_channels.values_mut() {
                debug!("Checking group channel: {:?}", group_channel);
                if let Some(latest_tick) = group_channel.latest_tick {
                    if tick - latest_tick > (i16::MAX / 2) {
                        debug!(
                    ?tick,
                    ?latest_tick,
                    ?group_channel,
                    "Setting the latest_tick tick to tick because there hasn't been any new updates in a while");
                        group_channel.latest_tick = Some(tick);
                    }
                }
            }
        }
    }
}
