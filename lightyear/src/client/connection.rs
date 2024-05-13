//! Specify how a Client sends/receives messages with a Server
use anyhow::{Context, Result};
use bevy::ecs::component::Tick as BevyTick;
use bevy::ecs::entity::{EntityHashMap, MapEntities};
use bevy::prelude::{Component, Entity, Local, Mut, Resource, World};
use bevy::reflect::Reflect;
use bevy::utils::{Duration, HashMap};
use bytes::Bytes;
use serde::Serialize;
use tracing::{debug, error, info, trace, trace_span, warn};

use crate::channel::builder::{EntityUpdatesChannel, PingChannel};
use bitcode::encoding::Fixed;

use crate::channel::senders::ChannelSend;
use crate::client::config::PacketConfig;
use crate::client::message::ClientMessage;
use crate::client::sync::SyncConfig;
use crate::inputs::native::input_buffer::InputBuffer;
use crate::packet::message_manager::MessageManager;
use crate::packet::packet::Packet;
use crate::packet::packet_manager::{Payload, PACKET_BUFFER_CAPACITY};
use crate::prelude::{Channel, ChannelKind, ClientId, Message, ReplicationGroup, TargetEntity};
use crate::protocol::channel::ChannelRegistry;
use crate::protocol::component::{ComponentNetId, ComponentRegistry};
use crate::protocol::message::MessageRegistry;
use crate::protocol::registry::NetId;
use crate::protocol::BitSerializable;
use crate::serialize::bitcode::reader::BufferPool;
use crate::serialize::bitcode::writer::BitcodeWriter;
use crate::serialize::reader::ReadBuffer;
use crate::serialize::writer::WriteBuffer;
use crate::serialize::RawData;
use crate::server::message::ServerMessage;
use crate::shared::events::connection::ConnectionEvents;
use crate::shared::message::MessageSend;
use crate::shared::ping::manager::{PingConfig, PingManager};
use crate::shared::ping::message::{Ping, Pong, SyncMessage};
use crate::shared::replication::components::{Replicate, ReplicationGroupId, ReplicationTarget};
use crate::shared::replication::network_target::NetworkTarget;
use crate::shared::replication::receive::ReplicationReceiver;
use crate::shared::replication::send::ReplicationSender;
use crate::shared::replication::systems::ReplicateCache;
use crate::shared::replication::{ReplicationMessage, ReplicationSend};
use crate::shared::replication::{ReplicationMessageData, ReplicationPeer, ReplicationReceive};
use crate::shared::sets::{ClientMarker, ServerMarker};
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
    pub(crate) replication_sender: ReplicationSender,
    pub(crate) replication_receiver: ReplicationReceiver,
    pub(crate) events: ConnectionEvents,
    pub ping_manager: PingManager,
    pub(crate) sync_manager: SyncManager,

    /// Stores some values that are needed to correctly replicate the despawning of Replicated entity.
    /// (when the entity is despawned, we don't have access to its components anymore, so we cache them here)
    pub(crate) replicate_component_cache: EntityHashMap<ReplicateCache>,

    /// Used to transfer raw bytes to a system that can convert the bytes to the actual type
    pub(crate) received_messages: HashMap<NetId, Vec<Bytes>>,
    writer: BitcodeWriter,
    pub(crate) reader_pool: BufferPool,
    // TODO: maybe don't do any replication until connection is synced?
}

impl ConnectionManager {
    pub(crate) fn new(
        component_registry: &ComponentRegistry,
        message_registry: &MessageRegistry,
        channel_registry: &ChannelRegistry,
        packet_config: PacketConfig,
        sync_config: SyncConfig,
        ping_config: PingConfig,
        input_delay_ticks: u16,
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
            component_registry: component_registry.clone(),
            message_registry: message_registry.clone(),
            message_manager,
            replication_sender,
            replication_receiver,
            ping_manager: PingManager::new(ping_config),
            sync_manager: SyncManager::new(sync_config, input_delay_ticks),
            replicate_component_cache: EntityHashMap::default(),
            events: ConnectionEvents::default(),
            received_messages: HashMap::default(),
            writer: BitcodeWriter::with_capacity(PACKET_BUFFER_CAPACITY),
            // TODO: it looks like we don't really need the pool this case, we can just keep re-using the same buffer
            reader_pool: BufferPool::new(1),
        }
    }

    #[doc(hidden)]
    /// Whether or not the connection is synced with the server
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

    pub(crate) fn update(&mut self, time_manager: &TimeManager, tick_manager: &TickManager) {
        self.message_manager
            .update(time_manager, &self.ping_manager, tick_manager);
        self.ping_manager.update(time_manager);

        // (we update the sync manager in POST_UPDATE)
    }

    fn send_ping(&mut self, ping: Ping) -> Result<()> {
        trace!("Sending ping {:?}", ping);
        self.writer.start_write();
        ClientMessage::Ping(ping).encode(&mut self.writer)?;
        let message_bytes = self.writer.finish_write().to_vec();
        self.message_manager
            .buffer_send(message_bytes, ChannelKind::of::<PingChannel>())?;
        Ok(())
    }

    fn send_pong(&mut self, pong: Pong) -> Result<()> {
        self.writer.start_write();
        ClientMessage::Pong(pong).encode(&mut self.writer)?;
        let message_bytes = self.writer.finish_write().to_vec();
        self.message_manager
            .buffer_send(message_bytes, ChannelKind::of::<PingChannel>())?;
        Ok(())
    }

    /// Send a message to the server
    pub fn send_message<C: Channel, M: Message>(&mut self, message: &M) -> Result<()> {
        self.send_message_to_target::<C, M>(message, NetworkTarget::None)
    }

    /// Send a message to the server, the message should be re-broadcasted according to the `target`
    pub fn send_message_to_target<C: Channel, M: Message>(
        &mut self,
        message: &M,
        target: NetworkTarget,
    ) -> Result<()> {
        self.erased_send_message_to_target(message, ChannelKind::of::<C>(), target)
    }

    fn erased_send_message_to_target<M: Message>(
        &mut self,
        message: &M,
        channel_kind: ChannelKind,
        target: NetworkTarget,
    ) -> Result<()> {
        let message_bytes = self.message_registry.serialize(message, &mut self.writer)?;
        self.buffer_message(message_bytes, channel_kind, target)
    }

    pub(crate) fn buffer_message(
        &mut self,
        message: RawData,
        channel: ChannelKind,
        target: NetworkTarget,
    ) -> Result<()> {
        // TODO: i know channel names never change so i should be able to get them as static
        // TODO: just have a channel registry enum as well?
        let channel_name = self
            .message_manager
            .channel_registry
            .name(&channel)
            .unwrap_or("unknown")
            .to_string();
        let message = ClientMessage::Message(message, target);
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
        // NOTE: this doesn't work too well because then duplicate actions/updates are accumulated before the connection is synced
        // if !self.sync_manager.is_synced() {
        //
        //
        //     // // clear the duplicate component checker
        //     // self.replication_sender.pending_unique_components.clear();
        //     return Ok(());
        // }

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
                trace!("Sending replication message: {:?}", message);
                // message.emit_send_logs(&channel_name);
                let message_id = self
                    .message_manager
                    .buffer_send_with_priority(message_bytes, channel, priority)?
                    .expect("The EntityUpdatesChannel should always return a message_id");

                // TODO: if should_track_ack OR bandwidth_cap is enabled
                // keep track of the group associated with the message, so we can handle receiving an ACK for that message_id later
                if should_track_ack {
                    self.replication_sender
                        .updates_message_id_to_group_id
                        .insert(message_id, (group_id, bevy_tick));
                }
                Ok(())
            })
    }

    /// Send packets that are ready to be sent
    pub(crate) fn send_packets(
        &mut self,
        time_manager: &TimeManager,
        tick_manager: &TickManager,
    ) -> Result<Vec<Payload>> {
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
                    Ok::<(), anyhow::Error>(())
                })?;
        }
        let payloads = self.message_manager.send_packets(tick_manager.tick());

        // update the replication sender about which messages were actually sent, and accumulate priority
        self.replication_sender.recv_send_notification();
        payloads
    }

    pub(crate) fn receive(
        &mut self,
        // TODO: use Commands to avoid blocking the world?
        world: &mut World,
        time_manager: &TimeManager,
        tick_manager: &TickManager,
    ) {
        let _span = trace_span!("receive").entered();
        for (channel_kind, messages) in self.message_manager.read_messages() {
            let channel_name = self
                .message_manager
                .channel_registry
                .name(&channel_kind)
                .unwrap_or("unknown");
            let _span_channel = trace_span!("channel", channel = channel_name).entered();

            if !messages.is_empty() {
                trace!(?channel_name, "Received messages");
                for (tick, single_data) in messages.into_iter() {
                    // TODO: in this case, it looks like we might not need the pool?
                    //  we can just have a single buffer, and keep re-using that buffer
                    trace!(pool_len = ?self.reader_pool.0.len(), "read from message manager");
                    let mut reader = self.reader_pool.start_read(single_data.as_ref());
                    // TODO: maybe just decode a single bit to know if it's message vs replication?
                    let message = ServerMessage::decode(&mut reader)
                        .expect("Could not decode server message");
                    // other message-handling logic
                    match message {
                        ServerMessage::Message(message) => {
                            // reset the reader to read the inner bytes
                            reader.reset_read(message.as_ref());
                            let net_id = reader
                                .decode::<NetId>(Fixed)
                                .expect("could not decode MessageKind");
                            self.received_messages
                                .entry(net_id)
                                .or_default()
                                .push(message.into());
                        }
                        ServerMessage::Replication(replication) => {
                            // buffer the replication message
                            self.replication_receiver.recv_message(replication, tick);
                        }
                        ServerMessage::Ping(ping) => {
                            // prepare a pong in response (but do not send yet, because we need
                            // to set the correct send time)
                            self.ping_manager
                                .buffer_pending_pong(&ping, time_manager.current_time());
                        }
                        ServerMessage::Pong(pong) => {
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
                                        "Updated server pong generation")
                        }
                    }

                    // return the buffer to the pool
                    self.reader_pool.attach(reader);
                }
            }
        }

        // NOTE: we run this outside of is_empty() because we could have received an update for a future tick that we can
        //  now apply. Also we can read from out buffers even if we didn't receive any messages.
        //
        // Check if we have any replication messages we can apply to the World (and emit events)
        if self.sync_manager.is_synced() {
            for (group, replication_list) in
                self.replication_receiver.read_messages(tick_manager.tick())
            {
                world.resource_scope(|world, component_registry: Mut<ComponentRegistry>| {
                    trace!(?group, ?replication_list, "read replication messages");
                    replication_list
                        .into_iter()
                        .for_each(|(tick, replication)| {
                            // TODO: we could include the server tick when this replication_message was sent.
                            self.replication_receiver.apply_world(
                                world,
                                component_registry.as_ref(),
                                tick,
                                replication,
                                group,
                                &mut self.events,
                            );
                        });
                })
            }
        }
    }

    pub(crate) fn recv_packet(&mut self, packet: Packet, tick_manager: &TickManager) -> Result<()> {
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
        self.replication_sender.recv_update_acks();
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
    fn writer(&mut self) -> &mut BitcodeWriter {
        &mut self.writer
    }

    fn new_connected_clients(&self) -> Vec<ClientId> {
        vec![]
    }

    fn prepare_entity_despawn(
        &mut self,
        entity: Entity,
        group: &ReplicationGroup,
        target: NetworkTarget,
    ) -> Result<()> {
        let group_id = group.group_id(Some(entity));
        // trace!(?entity, "Send entity despawn for tick {:?}", self.tick());
        let replication_sender = &mut self.replication_sender;
        replication_sender.prepare_entity_despawn(entity, group_id);
        Ok(())
    }

    fn prepare_component_insert(
        &mut self,
        entity: Entity,
        kind: ComponentNetId,
        component: RawData,
        component_registry: &ComponentRegistry,
        replication_target: &ReplicationTarget,
        group: &ReplicationGroup,
        target: NetworkTarget,
    ) -> Result<()> {
        let group_id = group.group_id(Some(entity));
        // debug!(
        //     ?entity,
        //     component = ?kind,
        //     tick = ?self.tick_manager.tick(),
        //     "Inserting single component"
        // );
        self.replication_sender
            .prepare_component_insert(entity, group_id, kind, component);
        Ok(())
    }

    fn prepare_component_remove(
        &mut self,
        entity: Entity,
        component_kind: ComponentNetId,
        group: &ReplicationGroup,
        target: NetworkTarget,
    ) -> Result<()> {
        let group_id = group.group_id(Some(entity));
        debug!(?entity, ?component_kind, "Sending RemoveComponent");
        self.replication_sender
            .prepare_component_remove(entity, group_id, component_kind);
        Ok(())
    }

    fn prepare_component_update(
        &mut self,
        entity: Entity,
        kind: ComponentNetId,
        component: RawData,
        group: &ReplicationGroup,
        target: NetworkTarget,
        component_change_tick: BevyTick,
        system_current_tick: BevyTick,
    ) -> Result<()> {
        let group_id = group.group_id(Some(entity));
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
                .prepare_entity_update(entity, group_id, kind, component);
        }
        Ok(())
    }

    fn buffer_replication_messages(&mut self, tick: Tick, bevy_tick: BevyTick) -> Result<()> {
        let _span = trace_span!("buffer_replication_messages").entered();
        self.buffer_replication_messages(tick, bevy_tick)
    }
    fn get_mut_replicate_cache(&mut self) -> &mut EntityHashMap<ReplicateCache> {
        &mut self.replicate_component_cache
    }
    fn cleanup(&mut self, tick: Tick) {
        debug!("Running replication clean");
        self.replication_sender.cleanup(tick);
    }
}
