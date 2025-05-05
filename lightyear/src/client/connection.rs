//! Specify how a Client sends/receives messages with a Server
#[cfg(not(feature = "std"))]
use alloc::{vec, vec::Vec};
use bevy::ecs::component::Tick as BevyTick;
use bevy::ecs::entity::MapEntities;
use bevy::prelude::{Resource, World};
use bytes::Bytes;
use core::time::Duration;
use tracing::{debug, trace, trace_span};

use crate::channel::builder::{
    EntityActionsChannel, EntityUpdatesChannel, PingChannel, PongChannel,
};

use crate::channel::receivers::ChannelReceive;
use crate::channel::senders::ChannelSend;
use crate::client::config::ClientConfig;
use crate::client::error::ClientError;
use crate::client::sync::SyncConfig;
use crate::connection::netcode::MAX_PACKET_SIZE;
use crate::packet::message_manager::MessageManager;
use crate::packet::packet_builder::{Payload, RecvPayload};
use crate::packet::priority_manager::PriorityConfig;
use crate::prelude::client::PredictionConfig;
use crate::prelude::{ChannelKind, ClientId, Message, MessageRegistry, ReplicationConfig};
use crate::protocol::channel::ChannelRegistry;
use crate::protocol::component::registry::ComponentRegistry;
use crate::protocol::registry::NetId;
use crate::serialize::reader::Reader;
use crate::serialize::writer::Writer;
use crate::serialize::{SerializationError, ToBytes};
use crate::server::error::ServerError;
use crate::shared::events::connection::ConnectionEvents;
use crate::shared::ping::manager::{PingConfig, PingManager};
use crate::shared::ping::message::{Ping, Pong};
use crate::shared::replication::delta::DeltaManager;
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
#[derive(Resource, Debug)]
pub struct ConnectionManager {
    pub(crate) message_registry: MessageRegistry,
    pub(crate) message_manager: MessageManager,
    pub(crate) delta_manager: DeltaManager,
    pub(crate) replication_sender: ReplicationSender,
    pub replication_receiver: ReplicationReceiver,
    pub(crate) events: ConnectionEvents,
    pub ping_manager: PingManager,
    pub(crate) sync_manager: SyncManager,
    /// Used to transfer raw bytes to a system that can convert the bytes to the actual type
    pub(crate) received_messages: Vec<(NetId, Bytes)>,
    pub(crate) writer: Writer,

    /// Internal buffer of the messages that we want to send.
    /// We use this so that:
    /// - in host server mode, we deserialize the bytes and push them to the server's Message Events queue directly
    /// - in non-host server mode, we buffer the bytes to the message manager as usual
    pub(crate) messages_to_send: Vec<(Bytes, ChannelKind)>,
}

// NOTE: useful when we sometimes need to create a temporary fake ConnectionManager
impl Default for ConnectionManager {
    fn default() -> Self {
        let replication_sender = ReplicationSender::new(
            crossbeam_channel::unbounded().1,
            crossbeam_channel::unbounded().1,
            crossbeam_channel::unbounded().1,
            ReplicationConfig::default(),
            false,
        );
        let replication_receiver = ReplicationReceiver::new();
        Self {
            message_registry: MessageRegistry::default(),
            message_manager: MessageManager::new(
                &ChannelRegistry::default(),
                0.0,
                PriorityConfig::default(),
            ),
            delta_manager: DeltaManager::default(),
            replication_sender,
            replication_receiver,
            ping_manager: PingManager::new(PingConfig::default()),
            sync_manager: SyncManager::new(SyncConfig::default(), PredictionConfig::default()),
            events: ConnectionEvents::default(),
            received_messages: Vec::default(),
            writer: Writer::with_capacity(0),
            messages_to_send: Vec::default(),
        }
    }
}

impl ConnectionManager {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        message_registry: &MessageRegistry,
        channel_registry: &ChannelRegistry,
        client_config: &ClientConfig,
    ) -> Self {
        let bandwidth_cap_enabled = client_config.packet.bandwidth_cap_enabled;
        // create the message manager and the channels
        let mut message_manager = MessageManager::new(
            channel_registry,
            client_config.packet.nack_rtt_multiple,
            client_config.packet.into(),
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
            client_config.replication,
            bandwidth_cap_enabled,
        );
        let replication_receiver = ReplicationReceiver::new();
        Self {
            message_registry: message_registry.clone(),
            message_manager,
            delta_manager: DeltaManager::default(),
            replication_sender,
            replication_receiver,
            ping_manager: PingManager::new(client_config.ping),
            sync_manager: SyncManager::new(client_config.sync, client_config.prediction),
            events: ConnectionEvents::default(),
            received_messages: Vec::default(),
            writer: Writer::with_capacity(MAX_PACKET_SIZE),
            messages_to_send: Vec::default(),
        }
    }

    /// Returns the current interpolation tick of the client
    pub fn interpolation_tick(&self, tick_manager: &TickManager) -> Tick {
        self.sync_manager.interpolation_tick(tick_manager)
    }

    #[doc(hidden)]
    /// Returns true if the connection is synced with the server
    pub fn is_synced(&self) -> bool {
        self.sync_manager.is_synced()
    }

    /// Amount of input delay applied
    pub(crate) fn input_delay_ticks(&self) -> u16 {
        self.sync_manager.current_input_delay
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
        let mut writer = Writer::with_capacity(ping.bytes_len());
        ping.to_bytes(&mut writer)?;
        let message_bytes = writer.to_bytes();
        self.message_manager
            .buffer_send(message_bytes, ChannelKind::of::<PingChannel>())?;
        Ok(())
    }

    fn send_pong(&mut self, pong: Pong) -> Result<(), ClientError> {
        let mut writer = Writer::with_capacity(pong.bytes_len());
        pong.to_bytes(&mut writer)?;
        let message_bytes = writer.to_bytes();
        self.message_manager
            .buffer_send(message_bytes, ChannelKind::of::<PongChannel>())?;
        Ok(())
    }

    // TODO: we need `&mut self` because MapEntities requires `&mut EntityMapper` even though it's not needed here
    /// Convert entities in the message to be compatible with the remote world
    pub fn map_entities_to_remote<M: Message + MapEntities>(&mut self, message: &mut M) {
        let mapper = &mut self.replication_receiver.remote_entity_map.local_to_remote;
        message.map_entities(mapper);
    }

    pub(crate) fn buffer_replication_messages(
        &mut self,
        tick: Tick,
        bevy_tick: BevyTick,
        time_manager: &TimeManager,
    ) -> Result<(), ClientError> {
        // NOTE: this doesn't work too well because then duplicate actions/updates are accumulated before the connection is synced
        // if !self.sync_manager.is_synced() {
        //
        //
        //     // // clear the duplicate component checker
        //     // self.replication_sender.pending_unique_components.clear();
        //     return Ok(());
        // }

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

    // TODO: alternative, if we are running in host-server mode,
    //  use a special local transport and still buffer things inside the MessageManager?
    /// Send packets that are ready to be sent.
    /// In host-server mode:
    /// - go through messages_to_send and make the server's ConnectionManager receive them
    pub(crate) fn send_packets_host_server(
        &mut self,
        local_client_id: ClientId,
        server_manager: &mut crate::server::connection::ConnectionManager,
    ) -> Result<(), ServerError> {
        // go through messages_to_send, deserialize them and make the server receive them
        self.messages_to_send
            .drain(..)
            .try_for_each(|(message_bytes, channel_kind)| {
                server_manager
                    .connection_mut(local_client_id)?
                    .receive_message(
                        Reader::from(message_bytes),
                        channel_kind,
                        &self.message_registry,
                    )
                    .map_err(ServerError::from)
            })?;
        Ok(())
    }

    /// Send packets that are ready to be sent.
    /// In non-host-server mode:
    /// - go through messages_to_send, buffer them to the message manager and then send packets that are ready
    pub(crate) fn send_packets(
        &mut self,
        time_manager: &TimeManager,
        tick_manager: &TickManager,
    ) -> Result<Vec<Payload>, ClientError> {
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
                // TODO: should we send real time or virtual time here?
                //  probably real time if we just want to estimate RTT?
                // update the send time of the pong
                pong.pong_sent_time = time_manager.current_time();
                self.send_pong(pong)?;
                Ok::<(), ClientError>(())
            })?;

        // buffer the messages into the message manager
        self.messages_to_send
            .drain(..)
            .try_for_each(|(message_bytes, channel_kind)| {
                self.message_manager
                    .buffer_send(message_bytes, channel_kind)?;
                Ok::<(), ClientError>(())
            })?;

        // get the payloads from the message manager
        let payloads = self.message_manager.send_packets(tick_manager.tick());

        // update the replication sender about which messages were actually sent, and accumulate priority
        self.replication_sender.recv_send_notification();
        payloads.map_err(Into::into)
    }

    pub(crate) fn receive(
        &mut self,
        world: &mut World,
        // TODO: pass the MessageRegistry` as arguments instead of storing a copy
        //  in the `ConnectionManager`
        component_registry: &mut ComponentRegistry,
        time_manager: &TimeManager,
        tick_manager: &TickManager,
    ) -> Result<(), ClientError> {
        let _span = trace_span!("receive").entered();
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

                    // TODO: put metric somewhere else?
                    #[cfg(feature = "metrics")]
                    {
                        metrics::counter!("message::received").increment(1);
                        // metrics::counter!("message::received", "channel" => channel.name.to_string()).increment(1);
                        metrics::counter!(format!("message::received::{}", channel.name))
                            .increment(1);
                    }

                    trace!(?channel_kind, ?tick, ?single_data, "Received message");
                    let mut reader = Reader::from(single_data);
                    if *channel_kind == ChannelKind::of::<PingChannel>() {
                        let ping = Ping::from_bytes(&mut reader)?;
                        // prepare a pong in response (but do not send yet, because we need
                        // to set the correct send time)
                        self.ping_manager
                            .buffer_pending_pong(&ping, time_manager.current_time());
                    } else if *channel_kind == ChannelKind::of::<PongChannel>() {
                        let pong = Pong::from_bytes(&mut reader)?;
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
                    } else if *channel_kind == ChannelKind::of::<EntityActionsChannel>() {
                        let actions = EntityActionsMessage::from_bytes(&mut reader)?;
                        self.replication_receiver.recv_actions(actions, tick);
                    } else if *channel_kind == ChannelKind::of::<EntityUpdatesChannel>() {
                        let updates = EntityUpdatesMessage::from_bytes(&mut reader)?;
                        self.replication_receiver.recv_updates(updates, tick);
                    } else {
                        // TODO: this code is copy-pasted from self.receive_message because of borrow checker limitations
                        let net_id = NetId::from_bytes(&mut reader)?;
                        let single_data = reader.consume();
                        self.received_messages.push((net_id, single_data));
                    }
                }
                Ok::<(), SerializationError>(())
            })?;

        if self.sync_manager.is_synced() {
            // Check if we have any replication messages we can apply to the World (and emit events)
            self.replication_receiver.apply_world(
                world,
                None,
                component_registry,
                tick_manager.tick(),
                &mut self.events,
            );
        }
        Ok(())
    }

    /// Receive a message from the server
    pub(crate) fn receive_message(&mut self, mut reader: Reader) -> Result<(), SerializationError> {
        // identify the type of message
        let net_id = NetId::from_bytes(&mut reader)?;
        let single_data = reader.consume();
        self.received_messages.push((net_id, single_data));
        Ok(())
    }

    pub(crate) fn recv_packet(
        &mut self,
        packet: RecvPayload,
        tick_manager: &TickManager,
        component_registry: &ComponentRegistry,
    ) -> Result<(), ClientError> {
        // receive the packets, buffer them, update any sender that were waiting for their sent messages to be acked
        let tick = self.message_manager.recv_packet(packet)?;
        trace!("Received server packet with tick: {:?}", tick);
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

    fn writer(&mut self) -> &mut Writer {
        &mut self.writer
    }

    fn new_connected_clients(&self) -> Vec<ClientId> {
        vec![]
    }

    fn cleanup(&mut self, tick: Tick) {
        debug!("Running replication clean");
        self.replication_sender.cleanup(tick);
        self.delta_manager.tick_cleanup(tick);
    }
}

#[cfg(test)]
mod tests {
    use crate::prelude::{client, server, ClientConnectionManager, RemoteEntityMap};
    use crate::tests::protocol::EntityMessage;
    use crate::tests::stepper::BevyStepper;

    /// Check that we can map entities from the local world to the remote world
    /// using the ConnectionManager
    #[test]
    fn test_map_entities_to_remote() {
        let mut stepper = BevyStepper::default();


        // spawn an entity on server
        let server_entity = stepper
            .server_app
            .world_mut()
            .spawn(server::Replicate::default())
            .id();
        stepper.frame_step();
        stepper.frame_step();

        // check that the entity was spawned
        let client_entity = stepper
            .client_app
            .world()
            .resource::<client::ConnectionManager>()
            .replication_receiver
            .remote_entity_map
            .get_local(server_entity)
            .expect("entity was not replicated to client");

        // spawn an entity on the client which contains a link to another entity
        // we need to map that entity to the remote world
        assert_eq!(
            *stepper
                .client_app
                .world_mut()
                .resource_mut::<ClientConnectionManager>()
                .replication_receiver
                .remote_entity_map
                .local_to_remote
                .get(&client_entity)
                .unwrap(),
            server_entity
        );

        let mut message = EntityMessage(client_entity);
        stepper
            .client_app
            .world_mut()
            .resource_mut::<ClientConnectionManager>()
            .map_entities_to_remote(&mut message);
        assert!(RemoteEntityMap::is_mapped(message.0));
        assert_eq!(RemoteEntityMap::mark_unmapped(message.0), server_entity);
    }
}
