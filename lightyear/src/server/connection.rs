//! Specify how a Server sends/receives messages with a Client
use bevy::ecs::component::Tick as BevyTick;
use bevy::ecs::entity::EntityHash;
use bevy::prelude::{Component, Entity, Mut, Resource, World};
use bevy::ptr::Ptr;
use bevy::utils::{HashMap, HashSet};
use bytes::Bytes;
use hashbrown::hash_map::Entry;
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
use crate::prelude::server::{DisconnectEvent, RoomId, RoomManager};
use crate::prelude::{
    Channel, ChannelKind, Message, PreSpawnedPlayerObject, ReplicationGroup, ShouldBePredicted,
};
use crate::protocol::channel::ChannelRegistry;
use crate::protocol::component::{
    ComponentError, ComponentKind, ComponentNetId, ComponentRegistry,
};
use crate::protocol::message::{MessageError, MessageRegistry, MessageType};
use crate::protocol::registry::NetId;
use crate::serialize::reader::Reader;
use crate::serialize::writer::Writer;
use crate::serialize::{SerializationError, ToBytes};
use crate::server::config::{PacketConfig, ReplicationConfig};
use crate::server::error::ServerError;
use crate::server::events::{ConnectEvent, ServerEvents};
use crate::server::replication::send::ReplicateCache;
use crate::server::visibility::error::VisibilityError;
use crate::shared::events::connection::ConnectionEvents;
use crate::shared::message::MessageSend;
use crate::shared::ping::manager::{PingConfig, PingManager};
use crate::shared::ping::message::{Ping, Pong};
use crate::shared::replication::components::ReplicationGroupId;
use crate::shared::replication::delta::DeltaManager;
use crate::shared::replication::network_target::NetworkTarget;
use crate::shared::replication::receive::ReplicationReceiver;
use crate::shared::replication::send::ReplicationSender;
use crate::shared::replication::{EntityActionsMessage, EntityUpdatesMessage, ReplicationPeer};
use crate::shared::replication::{ReplicationReceive, ReplicationSend};
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
            replicate_component_cache: EntityHashMap::default(),
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

    /// Queues up a message to be sent to all clients matching the specific [`NetworkTarget`]
    pub fn send_message_to_target<C: Channel, M: Message>(
        &mut self,
        message: &M,
        target: NetworkTarget,
    ) -> Result<(), ServerError> {
        self.erased_send_message_to_target(message, ChannelKind::of::<C>(), target)
    }

    /// Send a message to all clients in a room
    pub fn send_message_to_room<C: Channel, M: Message>(
        &mut self,
        message: &M,
        room_id: RoomId,
        room_manager: &RoomManager,
    ) -> Result<(), ServerError> {
        let room = room_manager
            .get_room(room_id)
            .ok_or::<ServerError>(VisibilityError::RoomIdNotFound(room_id).into())?;
        let target = NetworkTarget::Only(room.clients.iter().copied().collect());
        self.send_message_to_target::<C, M>(message, target)
    }

    /// Queues up a message to be sent to a client
    pub fn send_message<C: Channel, M: Message>(
        &mut self,
        client_id: ClientId,
        message: &M,
    ) -> Result<(), ServerError> {
        self.send_message_to_target::<C, M>(message, NetworkTarget::Only(vec![client_id]))
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
        message: Bytes,
        channel: ChannelKind,
        target: NetworkTarget,
    ) -> Result<(), ServerError> {
        self.connections
            .iter_mut()
            .filter(|(id, _)| target.targets(id))
            // NOTE: this clone is O(1), it just increments the reference count
            .try_for_each(|(_, c)| c.buffer_message(message.clone(), channel))
    }

    pub(crate) fn erased_send_message_to_target<M: Message>(
        &mut self,
        message: &M,
        channel_kind: ChannelKind,
        target: NetworkTarget,
    ) -> Result<(), ServerError> {
        self.message_registry.serialize(message, &mut self.writer)?;
        let message_bytes = self.writer.split();
        self.buffer_message(message_bytes, channel_kind, target)
    }

    /// Buffer all the replication messages to send.
    /// Keep track of the bevy Change Tick: when a message is acked, we know that we only have to send
    /// the updates since that Change Tick
    #[cfg_attr(feature = "trace", instrument(level = Level::INFO, skip_all))]
    pub(crate) fn buffer_replication_messages(
        &mut self,
        tick: Tick,
        bevy_tick: BevyTick,
    ) -> Result<(), ServerError> {
        let _span = info_span!("buffer_replication_messages").entered();
        self.connections
            .values_mut()
            .try_for_each(move |c| c.buffer_replication_messages(tick, bevy_tick))
    }

    #[cfg_attr(feature = "trace", instrument(level = Level::INFO, skip_all))]
    pub(crate) fn receive(
        &mut self,
        world: &mut World,
        time_manager: &TimeManager,
        tick_manager: &TickManager,
    ) -> Result<(), ServerError> {
        let mut messages_to_rebroadcast = vec![];
        // TODO: do this in parallel
        self.connections
            .iter_mut()
            .try_for_each(|(client_id, connection)| {
                let _span = trace_span!("receive", ?client_id).entered();
                world.resource_scope(|world, component_registry: Mut<ComponentRegistry>| {
                    // receive events on the connection
                    let events = connection.receive(
                        world,
                        component_registry.as_ref(),
                        time_manager,
                        tick_manager,
                    )?;
                    // move the events from the connection to the connection manager
                    self.events.push_events(*client_id, events);
                    Ok::<(), ServerError>(())
                })?;

                // rebroadcast messages
                messages_to_rebroadcast
                    .extend(std::mem::take(&mut connection.messages_to_rebroadcast));
                Ok::<(), ServerError>(())
            })?;
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
    ) -> Result<(), ServerError> {
        let net_id = component_registry
            .get_net_id::<C>()
            .ok_or::<ServerError>(ComponentError::NotRegistered.into())?;
        // We store the Bytes in a hashmap, maybe more efficient to write the replication message directly?
        component_registry.serialize(data, &mut self.writer)?;
        let raw_data = self.writer.split();
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
    writer: Writer,
    // messages that we have received that need to be rebroadcasted to other clients
    pub(crate) messages_to_rebroadcast: Vec<(Bytes, NetworkTarget, ChannelKind)>,
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
            writer: Writer::with_capacity(MAX_PACKET_SIZE),
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
    ) -> Result<(), ServerError> {
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
                    Ok::<(), ServerError>(())
                })?;
        }
        let payloads = self.message_manager.send_packets(tick_manager.tick())?;

        // update the replication sender about which messages were actually sent, and accumulate priority
        self.replication_sender.recv_send_notification();
        Ok(payloads)
    }

    pub fn receive(
        &mut self,
        world: &mut World,
        component_registry: &ComponentRegistry,
        time_manager: &TimeManager,
        tick_manager: &TickManager,
    ) -> Result<ConnectionEvents, ServerError> {
        let _span = trace_span!("receive").entered();
        let message_registry = world.resource::<MessageRegistry>();
        self.message_manager
            .channels
            .iter_mut()
            .try_for_each(|(channel_kind, channel)| {
                while let Some((tick, single_data)) = channel.receiver.read_message() {
                    // let channel_name = self
                    //     .message_manager
                    //     .channel_registry
                    //     .name(&channel_kind)
                    //     .unwrap_or("unknown");
                    // let _span_channel = trace_span!("channel", channel = channel_name).entered();

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
                        // TODO: we only get RawData here, does that mean we're deserializing multiple times?
                        //  instead just read the bytes for the target!!
                        let ClientMessage { message, target } =
                            ClientMessage::from_bytes(&mut reader)?;

                        let mut reader = Reader::from(message);
                        let net_id = NetId::from_bytes(&mut reader)?;
                        // we are also sending target and channel kind so the message can be
                        // rebroadcasted to other clients after we have converted the entities from the
                        // client World to the server World
                        // TODO: but do we have data to convert the entities from the client to the server?
                        //  I don't think so... maybe the sender should map_entities themselves?
                        //  or it matters for input messages?
                        // TODO: avoid clone with Arc<[u8]>?
                        let data = (reader.consume(), target.clone(), *channel_kind);

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
        Ok(std::mem::replace(&mut self.events, ConnectionEvents::new()))
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
        debug!("Received server packet with tick: {:?}", tick);
        Ok(())
    }
}

impl ConnectionManager {
    pub(crate) fn prepare_entity_despawn(
        &mut self,
        entity: Entity,
        group_id: ReplicationGroupId,
        target: NetworkTarget,
    ) -> Result<(), ServerError> {
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
    ) -> Result<(), ServerError> {
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
        prediction_target: Option<&NetworkTarget>,
        group_id: ReplicationGroupId,
        target: NetworkTarget,
        delta_compression: bool,
        tick: Tick,
        bevy_tick: BevyTick,
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

        // same thing for PreSpawnedPlayerObject: that component should only be replicated to prediction_target
        let mut actual_target = target;
        let should_be_predicted_kind = ComponentKind::of::<ShouldBePredicted>();
        let pre_spawned_player_object_kind = ComponentKind::of::<PreSpawnedPlayerObject>();
        if kind == should_be_predicted_kind || kind == pre_spawned_player_object_kind {
            actual_target = prediction_target.unwrap().clone();
        }

        // even with delta-compression enabled
        // the diff can be shared for every client since we're inserting
        if delta_compression {
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
                    .serialize_diff_from_base_value(component_data, &mut self.writer, kind)
                    .expect("could not serialize delta")
            }
        } else {
            component_registry
                .erased_serialize(component_data, &mut self.writer, kind)
                .expect("could not serialize component")
        };
        let raw_data = self.writer.split();
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
        group_id: ReplicationGroupId,
        target: NetworkTarget,
        component_change_tick: BevyTick,
        system_current_tick: BevyTick,
        tick: Tick,
        delta_compression: bool,
    ) -> Result<(), ServerError> {
        trace!(
            ?kind,
            ?entity,
            ?component_change_tick,
            ?system_current_tick,
            "Prepare entity update"
        );
        let mut raw_data: Bytes = Bytes::new();
        if !delta_compression {
            // we serialize once and re-use the result for all clients
            registry.erased_serialize(component, &mut self.writer, kind)?;
            raw_data = self.writer.split();
        }
        let mut num_targets = 0;
        self.apply_replication(target).try_for_each(|client_id| {
            // TODO: should we have additional state tracking so that we know we are in the process of sending this entity to clients?
            let replication_sender = &mut self.connections.get_mut(&client_id).ok_or(ServerError::ClientIdNotFound(client_id))?.replication_sender;
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

impl MessageSend for ConnectionManager {
    type Error = ServerError;
    fn send_message_to_target<C: Channel, M: Message>(
        &mut self,
        message: &M,
        target: NetworkTarget,
    ) -> Result<(), ServerError> {
        self.send_message_to_target::<C, M>(message, target)
    }

    fn erased_send_message_to_target<M: Message>(
        &mut self,
        message: &M,
        channel_kind: ChannelKind,
        target: NetworkTarget,
    ) -> Result<(), ServerError> {
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
    type Error = ServerError;
    type ReplicateCache = EntityHashMap<Entity, ReplicateCache>;

    fn writer(&mut self) -> &mut Writer {
        &mut self.writer
    }

    fn new_connected_clients(&self) -> Vec<ClientId> {
        self.new_clients.clone()
    }

    fn replication_cache(&mut self) -> &mut Self::ReplicateCache {
        &mut self.replicate_component_cache
    }

    /// Buffer the replication messages
    fn buffer_replication_messages(
        &mut self,
        tick: Tick,
        bevy_tick: BevyTick,
    ) -> Result<(), ServerError> {
        self.buffer_replication_messages(tick, bevy_tick)
    }

    fn cleanup(&mut self, tick: Tick) {
        debug!("Running replication send cleanup");
        for connection in self.connections.values_mut() {
            connection.replication_sender.cleanup(tick);
        }
    }
}
