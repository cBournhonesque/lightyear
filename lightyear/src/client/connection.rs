//! Specify how a Client sends/receives messages with a Server
use bevy::ecs::component::Tick as BevyTick;
use bevy::ecs::entity::EntityHashMap;
use bevy::prelude::{Mut, Resource, World};
use bevy::utils::{Duration, HashMap};
use bytes::Bytes;
use tracing::{debug, trace, trace_span};

use crate::channel::builder::{
    EntityActionsChannel, EntityUpdatesChannel, PingChannel, PongChannel,
};
use bitcode::encoding::Fixed;

use crate::channel::senders::ChannelSend;
use crate::client::config::{PacketConfig, ReplicationConfig};
use crate::client::error::ClientError;
use crate::client::message::ClientMessage;
use crate::client::replication::send::ReplicateCache;
use crate::client::sync::SyncConfig;
use crate::packet::message_manager::MessageManager;
use crate::packet::packet_builder::{Payload, PACKET_BUFFER_CAPACITY};
use crate::prelude::{Channel, ChannelKind, ClientId, Message};
use crate::protocol::channel::ChannelRegistry;
use crate::protocol::component::ComponentRegistry;
use crate::protocol::message::{MessageError, MessageRegistry, MessageType};
use crate::protocol::registry::NetId;
use crate::protocol::BitSerializable;
use crate::serialize::bitcode::reader::BufferPool;
use crate::serialize::bitcode::writer::BitcodeWriter;
use crate::serialize::reader::ReadBuffer;
use crate::serialize::writer::WriteBuffer;
use crate::serialize::RawData;
use crate::shared::events::connection::ConnectionEvents;
use crate::shared::message::MessageSend;
use crate::shared::ping::manager::{PingConfig, PingManager};
use crate::shared::ping::message::{Ping, Pong};
use crate::shared::replication::delta::DeltaManager;
use crate::shared::replication::network_target::NetworkTarget;
use crate::shared::replication::receive::ReplicationReceiver;
use crate::shared::replication::send::ReplicationSender;
use crate::shared::replication::{EntityActionsMessage, EntityUpdatesMessage, ReplicationSend};
use crate::shared::replication::{ReplicationPeer, ReplicationReceive};
use crate::shared::sets::ClientMarker;
use crate::shared::tick_manager::Tick;
use crate::shared::tick_manager::TickManager;
use crate::shared::time_manager::TimeManager;

use super::sync::SyncManager;

/// Wrapper that handles the connection with the server
///
/// This is the main [`Resource`] to use to interact with the server (send inputs, messages, etc.)
///
/// ```rust,ignore
/// # use bevy::prelude::*;
/// # use lightyear::client::connection::ConnectionManager as ClientConnectionManager;
/// use lightyear::prelude::NetworkTarget;
/// fn my_system(
///   tick_manager: Res<TickManager>,
///   mut connection: ResMut<ClientConnectionManager>
/// ) {
///    // send a message to the server
///    connection.send_message::<MyChannel, MyMessage>("Hello, server!");
///    // send a message to some other client with ClientId 2
///    connection.send_message_to_target::<MyChannel, MyMessage>("Hello, server!", NetworkTarget::Single(2));
/// }
/// ```
#[derive(Resource)]
pub struct ConnectionManager {
    pub(crate) component_registry: ComponentRegistry,
    pub(crate) message_registry: MessageRegistry,
    pub(crate) message_manager: MessageManager,
    pub(crate) delta_manager: DeltaManager,
    pub(crate) replication_sender: ReplicationSender,
    pub(crate) replication_receiver: ReplicationReceiver,
    pub(crate) events: ConnectionEvents,
    pub ping_manager: PingManager,
    pub(crate) sync_manager: SyncManager,

    /// Stores some values that are needed to correctly replicate the despawning of Replicated entity.
    /// (when the entity is despawned, we don't have access to its components anymore, so we cache them here)
    pub(crate) replicate_component_cache: EntityHashMap<ReplicateCache>,

    /// Used to read the leafwing InputMessages from other clients
    #[cfg(feature = "leafwing")]
    pub(crate) received_leafwing_input_messages: HashMap<NetId, Vec<Bytes>>,
    /// Used to transfer raw bytes to a system that can convert the bytes to the actual type
    pub(crate) received_messages: HashMap<NetId, Vec<Bytes>>,
    pub(crate) writer: BitcodeWriter,
    pub(crate) reader_pool: BufferPool,
    // TODO: maybe don't do any replication until connection is synced?
}

impl ConnectionManager {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        component_registry: &ComponentRegistry,
        message_registry: &MessageRegistry,
        channel_registry: &ChannelRegistry,
        replication_config: ReplicationConfig,
        packet_config: PacketConfig,
        sync_config: SyncConfig,
        ping_config: PingConfig,
        input_delay_ticks: u16,
    ) -> Self {
        let bandwidth_cap_enabled = packet_config.bandwidth_cap_enabled;
        // create the message manager and the channels
        let mut message_manager = MessageManager::new(
            channel_registry,
            packet_config.nack_rtt_multiple,
            packet_config.into(),
        );
        // get notified when a replication-update message gets acked/nacked
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
            component_registry: component_registry.clone(),
            message_registry: message_registry.clone(),
            message_manager,
            delta_manager: DeltaManager::default(),
            replication_sender,
            replication_receiver,
            ping_manager: PingManager::new(ping_config),
            sync_manager: SyncManager::new(sync_config, input_delay_ticks),
            replicate_component_cache: EntityHashMap::default(),
            events: ConnectionEvents::default(),
            #[cfg(feature = "leafwing")]
            received_leafwing_input_messages: HashMap::default(),
            received_messages: HashMap::default(),
            writer: BitcodeWriter::with_capacity(PACKET_BUFFER_CAPACITY),
            // TODO: it looks like we don't really need the pool this case, we can just keep re-using the same buffer
            reader_pool: BufferPool::new(1),
        }
    }

    #[doc(hidden)]
    /// Returns true if the connection is synced with the server
    pub fn is_synced(&self) -> bool {
        self.sync_manager.is_synced()
    }

    /// Returns true if we received a new server packet on this frame
    pub(crate) fn received_new_server_tick(&self) -> bool {
        self.sync_manager.duration_since_latest_received_server_tick == Duration::default()
    }

    /// The latest server tick that we received from the server.
    pub(crate) fn latest_received_server_tick(&self) -> Tick {
        self.sync_manager
            .latest_received_server_tick
            .unwrap_or(Tick(0))
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

        // (we update the sync manager in POST_UPDATE)
    }

    fn send_ping(&mut self, ping: Ping) -> Result<(), ClientError> {
        trace!("Sending ping {:?}", ping);
        self.writer.start_write();
        self.writer.encode(&ping, Fixed)?;
        let message_bytes = self.writer.finish_write().to_vec();
        self.message_manager
            .buffer_send(message_bytes, ChannelKind::of::<PingChannel>())?;
        Ok(())
    }

    fn send_pong(&mut self, pong: Pong) -> Result<(), ClientError> {
        self.writer.start_write();
        self.writer.encode(&pong, Fixed)?;
        let message_bytes = self.writer.finish_write().to_vec();
        self.message_manager
            .buffer_send(message_bytes, ChannelKind::of::<PingChannel>())?;
        Ok(())
    }

    /// Send a message to the server
    pub fn send_message<C: Channel, M: Message>(&mut self, message: &M) -> Result<(), ClientError> {
        self.send_message_to_target::<C, M>(message, NetworkTarget::None)
    }

    /// Send a message to the server, the message should be re-broadcasted according to the `target`
    pub fn send_message_to_target<C: Channel, M: Message>(
        &mut self,
        message: &M,
        target: NetworkTarget,
    ) -> Result<(), ClientError> {
        self.erased_send_message_to_target(message, ChannelKind::of::<C>(), target)
    }

    fn erased_send_message_to_target<M: Message>(
        &mut self,
        message: &M,
        channel_kind: ChannelKind,
        target: NetworkTarget,
    ) -> Result<(), ClientError> {
        let message_bytes = self.message_registry.serialize(message, &mut self.writer)?;
        self.buffer_message(message_bytes, channel_kind, target)
    }

    pub(crate) fn buffer_message(
        &mut self,
        message: RawData,
        channel: ChannelKind,
        target: NetworkTarget,
    ) -> Result<(), ClientError> {
        // TODO: i know channel names never change so i should be able to get them as static
        let channel_name = self
            .message_manager
            .channel_registry
            .name(&channel)
            .ok_or::<ClientError>(MessageError::NotRegistered.into())?;
        let message = ClientMessage { message, target };
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
    ) -> Result<(), ClientError> {
        // NOTE: this doesn't work too well because then duplicate actions/updates are accumulated before the connection is synced
        // if !self.sync_manager.is_synced() {
        //
        //
        //     // // clear the duplicate component checker
        //     // self.replication_sender.pending_unique_components.clear();
        //     return Ok(());
        // }

        self.replication_sender
            .actions_to_send(tick, bevy_tick)
            .try_for_each(|(message, priority)| {
                // message.emit_send_logs("EntityActionsChannel");
                self.writer.start_write();
                message.encode(&mut self.writer)?;
                // TODO: doesn't this serialize the bytes twice?
                let message_bytes = self.writer.finish_write().to_vec();
                let message_id = self
                    .message_manager
                    // TODO: use const type_id?
                    .buffer_send_with_priority(
                        message_bytes,
                        ChannelKind::of::<EntityActionsChannel>(),
                        priority,
                    )?
                    .expect("The entity actions channels should always return a message_id");
                Ok::<(), ClientError>(())
            })?;

        self.replication_sender
            .updates_to_send(tick, bevy_tick)
            .try_for_each(|(message, priority)| {
                // message.emit_send_logs("EntityUpdatesChannel");
                self.writer.start_write();
                message.encode(&mut self.writer)?;
                let message_bytes = self.writer.finish_write().to_vec();
                let message_id = self
                    .message_manager
                    // TODO: use const type_id?
                    .buffer_send_with_priority(
                        message_bytes,
                        ChannelKind::of::<EntityUpdatesChannel>(),
                        priority,
                    )?
                    .expect("The entity actions channels should always return a message_id");

                // keep track of the group associated with the message, so we can handle receiving an ACK for that message_id later
                self.replication_sender.buffer_replication_update_message(
                    message.group_id,
                    message_id,
                    bevy_tick,
                    tick,
                );
                Ok(())
            })
    }

    /// Send packets that are ready to be sent
    pub(crate) fn send_packets(
        &mut self,
        time_manager: &TimeManager,
        tick_manager: &TickManager,
    ) -> Result<Vec<Payload>, ClientError> {
        // update the ping manager with the actual send time
        // TODO: issues here: we would like to send the ping/pong messages immediately, otherwise the recorded current time is incorrect
        //   - can give infinity priority to this channel?
        //   - can write directly to io otherwise?
        if time_manager.is_client_ready_to_send() {
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
                    // TODO: should we send real time or virtual time here?
                    //  probably real time if we just want to estimate RTT?
                    // update the send time of the pong
                    pong.pong_sent_time = time_manager.current_time();
                    self.send_pong(pong)?;
                    Ok::<(), ClientError>(())
                })?;
        }
        let payloads = self.message_manager.send_packets(tick_manager.tick());

        // update the replication sender about which messages were actually sent, and accumulate priority
        self.replication_sender.recv_send_notification();
        payloads.map_err(Into::into)
    }

    pub(crate) fn receive(
        &mut self,
        // TODO: use Commands to avoid blocking the world?
        world: &mut World,
        time_manager: &TimeManager,
        tick_manager: &TickManager,
    ) {
        let _span = trace_span!("receive").entered();
        let message_registry = world.resource::<MessageRegistry>();
        for (channel_kind, (tick, single_data)) in self.message_manager.read_messages() {
            // let channel_name = self
            //     .message_manager
            //     .channel_registry
            //     .name(&channel_kind)
            //     .unwrap_or("unknown");
            // let _span_channel = trace_span!("channel", channel = channel_name).entered();

            trace!(?channel_kind, ?tick, ?single_data, "Received message");
            // TODO: in this case, it looks like we might not need the pool?
            //  we can just have a single buffer, and keep re-using that buffer
            trace!(pool_len = ?self.reader_pool.0.len(), "read from message manager");
            let mut reader = self.reader_pool.start_read(single_data.as_ref());
            if channel_kind == ChannelKind::of::<PingChannel>() {
                let ping = reader
                    .decode::<Ping>(Fixed)
                    .expect("Could not decode ping message");
                // prepare a pong in response (but do not send yet, because we need
                // to set the correct send time)
                self.ping_manager
                    .buffer_pending_pong(&ping, time_manager.current_time());
            } else if channel_kind == ChannelKind::of::<PongChannel>() {
                let pong = reader
                    .decode::<Pong>(Fixed)
                    .expect("Could not decode pong message");
                // process the pong
                self.ping_manager
                    .process_pong(&pong, time_manager.current_time());
                // TODO: a bit dangerous because we want:
                // - real time when computing RTT
                // - virtual time when computing the generation
                // - maybe we should just send both in Pong message?
                // update the tick generation from the time + tick information
                self.sync_manager.server_pong_tick = tick;
                self.sync_manager.server_pong_generation = pong
                    .pong_sent_time
                    .tick_generation(tick_manager.config.tick_duration, tick);
                trace!(
                    ?tick,
                    generation = ?self.sync_manager.server_pong_generation,
                    time = ?pong.pong_sent_time,
                    "Updated server pong generation"
                )
            } else if channel_kind == ChannelKind::of::<EntityActionsChannel>() {
                let actions = EntityActionsMessage::decode(&mut reader)
                    .expect("Could not decode EntityActionsMessage");
                self.replication_receiver.recv_actions(actions, tick);
            } else if channel_kind == ChannelKind::of::<EntityUpdatesChannel>() {
                let updates = EntityUpdatesMessage::decode(&mut reader)
                    .expect("Could not decode EntityUpdatesMessage");
                self.replication_receiver.recv_updates(updates, tick);
            } else {
                // identify the type of message
                let net_id = reader
                    .decode::<NetId>(Fixed)
                    .expect("could not decode MessageKind");
                match message_registry.message_type(net_id) {
                    #[cfg(feature = "leafwing")]
                    MessageType::LeafwingInput => {
                        self.received_leafwing_input_messages
                            .entry(net_id)
                            .or_default()
                            .push(single_data.into());
                    }
                    MessageType::NativeInput => {
                        todo!()
                    }
                    MessageType::Normal => {
                        self.received_messages
                            .entry(net_id)
                            .or_default()
                            .push(single_data.into());
                    }
                }
            }

            // return the buffer to the pool
            self.reader_pool.attach(reader);
        }

        if self.sync_manager.is_synced() {
            world.resource_scope(|world, component_registry: Mut<ComponentRegistry>| {
                // Check if we have any replication messages we can apply to the World (and emit events)
                self.replication_receiver.apply_world(
                    world,
                    None,
                    component_registry.as_ref(),
                    tick_manager.tick(),
                    &mut self.events,
                );
            });
        }
    }

    pub(crate) fn recv_packet(
        &mut self,
        packet: Payload,
        tick_manager: &TickManager,
        component_registry: &ComponentRegistry,
    ) -> Result<(), ClientError> {
        // receive the packets, buffer them, update any sender that were waiting for their sent messages to be acked
        let tick = self.message_manager.recv_packet(packet)?;
        debug!("Received server packet with tick: {:?}", tick);
        if self
            .sync_manager
            .latest_received_server_tick
            .map_or(true, |server_tick| tick >= server_tick)
        {
            trace!("new last recv server tick: {:?}", tick);
            self.sync_manager.latest_received_server_tick = Some(tick);
            // TODO: add 'received_new_server_tick' ?
            // we probably actually physically received the packet some time between our last `receive` and now.
            // Let's add delta / 2 as a compromise
            self.sync_manager.duration_since_latest_received_server_tick = Duration::default();
            // self.sync_manager.duration_since_latest_received_server_tick = time_manager.delta() / 2;
            self.sync_manager.update_server_time_estimate(
                tick_manager.config.tick_duration,
                self.ping_manager.rtt(),
            );
        }
        trace!(?tick, last_server_tick = ?self.sync_manager.latest_received_server_tick, "Recv server packet");
        // notify the replication sender that some sent messages were received
        self.replication_sender
            .recv_update_acks(component_registry, &mut self.delta_manager);
        Ok(())
    }
}

impl MessageSend for ConnectionManager {
    type Error = ClientError;
    fn send_message_to_target<C: Channel, M: Message>(
        &mut self,
        message: &M,
        target: NetworkTarget,
    ) -> Result<(), ClientError> {
        self.send_message_to_target::<C, M>(message, target)
    }

    fn erased_send_message_to_target<M: Message>(
        &mut self,
        message: &M,
        channel_kind: ChannelKind,
        target: NetworkTarget,
    ) -> Result<(), ClientError> {
        self.erased_send_message_to_target(message, channel_kind, target)
    }
}

impl ReplicationPeer for ConnectionManager {
    type Events = ConnectionEvents;
    type EventContext = ();
    type SetMarker = ClientMarker;
}

impl ReplicationReceive for ConnectionManager {
    fn events(&mut self) -> &mut Self::Events {
        &mut self.events
    }

    fn cleanup(&mut self, tick: Tick) {
        self.replication_receiver.cleanup(tick);
    }
}

impl ReplicationSend for ConnectionManager {
    type Error = ClientError;
    type ReplicateCache = EntityHashMap<ReplicateCache>;

    fn writer(&mut self) -> &mut BitcodeWriter {
        &mut self.writer
    }

    fn new_connected_clients(&self) -> Vec<ClientId> {
        vec![]
    }

    fn replication_cache(&mut self) -> &mut Self::ReplicateCache {
        &mut self.replicate_component_cache
    }

    fn buffer_replication_messages(
        &mut self,
        tick: Tick,
        bevy_tick: BevyTick,
    ) -> Result<(), ClientError> {
        let _span = trace_span!("buffer_replication_messages").entered();
        self.buffer_replication_messages(tick, bevy_tick)
    }
    fn cleanup(&mut self, tick: Tick) {
        debug!("Running replication clean");
        self.replication_sender.cleanup(tick);
        self.delta_manager.tick_cleanup(tick);
    }
}
