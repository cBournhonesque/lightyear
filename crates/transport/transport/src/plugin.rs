use crate::channel::builder::Transport;
use crate::channel::receivers::ChannelReceive;
use crate::channel::registry::{ChannelId, ChannelRegistry};
use crate::channel::senders::{ChannelSend, SendFlushOutcome};
use crate::error::TransportError;
use crate::packet::compression::decompress_payload;
use crate::packet::error::PacketError;
use crate::packet::header::PacketHeader;
use crate::packet::message::{FragmentData, MessageAck, ReceiveMessage, SingleData};
use crate::packet::packet_type::PacketType;
#[cfg(feature = "test_utils")]
use crate::prelude::{AppChannelExt, ChannelMode, ChannelSettings};
use bevy_app::prelude::*;
use bevy_ecs::prelude::*;
use bevy_ecs::schedule::IntoScheduleConfigs;
use bevy_platform::collections::hash_map::Entry;
use bevy_time::{Real, Time};
#[cfg(feature = "test_utils")]
use bevy_utils::default;
use bytes::Bytes;
use core::time::Duration;
use lightyear_connection::host::HostClient;
#[cfg(any(feature = "client", feature = "server"))]
use lightyear_connection::prelude::Disconnected;
use lightyear_core::prelude::LocalTimeline;
use lightyear_core::tick::Tick;
use lightyear_link::{Link, LinkPlugin, LinkSystems, Linked};
use lightyear_serde::reader::{ReadInteger, Reader};
use lightyear_serde::{SerializationError, ToBytes};
#[cfg(feature = "metrics")]
use lightyear_utils::metrics::TimerGauge;
#[allow(unused_imports)]
use tracing::{debug, error, info, trace, warn};

#[deprecated(note = "Use TransportSystems instead")]
pub type TransportSet = TransportSystems;

#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone, Copy)]
pub enum TransportSystems {
    // PRE UPDATE
    /// Receive messages from the Link and buffer them into the ChannelReceivers
    Receive,

    // PostUpdate
    /// Flush the messages buffered in the ChannelSenders to the Link
    Send,
}

/// Event triggered on a [`Transport`] entity when it receives a new packet
#[derive(EntityEvent)]
pub struct PacketReceived {
    pub entity: Entity,
    pub remote_tick: Tick,
}

/// Event triggered on a [`Transport`] entity when a sent packet is acknowledged.
#[derive(EntityEvent)]
pub struct PacketAcked {
    pub entity: Entity,
    pub packet_id: u32,
    pub rtt_sample: Duration,
}

/// Event triggered on a [`Transport`] entity when a sent packet is presumed lost
/// (its acknowledgement did not arrive before the nack timeout).
#[derive(EntityEvent)]
pub struct PacketLost {
    pub entity: Entity,
    pub packet_id: u32,
}

pub struct TransportPlugin;

impl TransportPlugin {
    /// Receives packets from the [`Link`],
    /// Depending on the [`ChannelId`], buffer the messages in the packet
    /// in the appropriate channel receiver
    fn buffer_receive(
        time: Res<Time<Real>>,
        #[cfg(feature = "std")] par_commands: ParallelCommands,
        #[cfg(not(feature = "std"))] mut commands: Commands,
        channel_registry: Res<ChannelRegistry>,
        mut query: Query<(Entity, &mut Link, &mut Transport), (With<Linked>, Without<HostClient>)>,
    ) {
        #[cfg(feature = "metrics")]
        let _timer = TimerGauge::new("transport/recv");

        #[cfg(feature = "std")]
        let query = query.par_iter_mut();
        #[cfg(not(feature = "std"))]
        let query = query.iter_mut();

        query
            .for_each(|(entity, mut link, mut transport)| {
                // enable split borrows
                let transport = &mut *transport;
                // update with the latest time
                transport.senders.values_mut().for_each(|sender_metadata| {
                    sender_metadata.sender.update(&time, &link.stats);
                    sender_metadata.message_acks.clear();
                    sender_metadata.message_nacks.clear();
                    sender_metadata.messages_sent.clear();
                });
                transport
                    .receivers
                    .values_mut()
                    .for_each(|receiver_metadata| {
                        receiver_metadata.receiver.update(time.elapsed());
                    });
                // check which packets were lost
                transport
                    .packet_manager
                    .header_manager
                    .update(time.elapsed(), &link.stats);
                transport
                    .packet_manager
                    .header_manager
                    .lost_packets
                    .drain(..)
                    .try_for_each(|lost_packet| {
                        #[cfg(feature = "metrics")]
                        metrics::counter!("transport/packets_lost").increment(1);
                        trace!(
                            target: "lightyear_debug::transport",
                            kind = "packet_lost",
                            schedule = "PreUpdate",
                            sample_point = "PreUpdate",
                            entity = ?entity,
                            packet_id = ?lost_packet,
                            packet_loss = true,
                            "transport packet marked lost"
                        );
                        #[cfg(feature = "std")]
                        par_commands.command_scope(|mut commands| {
                            commands.trigger(PacketLost {
                                entity,
                                packet_id: lost_packet.0,
                            });
                        });
                        #[cfg(not(feature = "std"))]
                        commands.trigger(PacketLost {
                            entity,
                            packet_id: lost_packet.0,
                        });
                        if let Some(message_map) =
                            transport.packet_to_message_map.remove(&lost_packet)
                        {
                            for (channel_kind, message_ack) in message_map {
                                let sender_metadata = transport
                                    .senders
                                    .get_mut(&channel_kind)
                                    .ok_or(PacketError::ChannelNotFound)?;
                                // TODO: batch the messages?
                                trace!(
                                    ?lost_packet,
                                    ?channel_kind,
                                    "message lost: {:?}",
                                    message_ack.message_id
                                );
                                sender_metadata.message_nacks.push(message_ack.message_id);
                                if message_ack.fragment_id.is_some() {
                                    transport.fragment_acks.remove(&message_ack.message_id);
                                }
                            }
                        }
                        Ok::<(), TransportError>(())
                    })
                    .ok();

                link.recv
                    .drain()
                    .try_for_each(|packet| {
                        let packet_len = packet.len();
                        #[cfg(feature = "metrics")]
                        metrics::gauge!("transport/recv_bytes").increment(packet_len as f64);

                        let mut cursor = Reader::from(packet);

                        // Parse the packet
                        let header = PacketHeader::from_bytes(&mut cursor)?;
                        let tick = header.tick;
                        trace!(
                            target: "lightyear_debug::transport",
                            kind = "packet_recv",
                            schedule = "PreUpdate",
                            sample_point = "PreUpdate",
                            entity = ?entity,
                            packet_id = ?header.packet_id,
                            remote_tick = tick.0,
                            bytes = packet_len,
                            packet_type = ?header.get_packet_type(),
                            "received transport packet"
                        );

                        // Update the packet acks before triggering PacketReceived, so timeline
                        // observers can consume RTT samples from this packet first.
                        let newly_acked_packets = transport
                            .packet_manager
                            .header_manager
                            .process_recv_packet_header(&header, time.elapsed());

                        #[cfg(feature = "std")]
                        par_commands.command_scope(|mut commands| {
                            newly_acked_packets.iter().for_each(|(packet_id, rtt_sample)| {
                                commands.trigger(PacketAcked {
                                    entity,
                                    packet_id: packet_id.0,
                                    rtt_sample: *rtt_sample,
                                });
                            });
                            commands.trigger(PacketReceived { entity, remote_tick: tick });
                        });
                        #[cfg(not(feature = "std"))]
                        {
                            newly_acked_packets.iter().for_each(|(packet_id, rtt_sample)| {
                                commands.trigger(PacketAcked {
                                    entity,
                                    packet_id: packet_id.0,
                                    rtt_sample: *rtt_sample,
                                });
                            });
                            commands.trigger(PacketReceived { entity, remote_tick: tick });
                        }

                        let mut packet_type = header.get_packet_type();
                        if packet_type.is_compressed() {
                            let compressed_payload = cursor.split();
                            let decompressed_payload =
                                decompress_payload(compressed_payload.as_ref(), transport.compression)?;
                            cursor = Reader::from(decompressed_payload);
                            packet_type = packet_type.uncompressed_variant();
                        }

                        // Parse the payload into messages, put them in the internal buffers for each channel
                        // we read directly from the packet and don't create intermediary datastructures to avoid allocations
                        // TODO: maybe do this in a helper function?
                        if packet_type == PacketType::DataFragment {
                            // read the fragment data
                            let channel_id = ChannelId::from_bytes(&mut cursor)?;
                            let fragment_data = FragmentData::from_bytes(&mut cursor)?;
                            let channel_name = channel_registry.get_name_from_net_id(channel_id);
                            #[cfg(feature = "metrics")]
                            {
                                metrics::gauge!("channel/recv_messages", "channel" => channel_name).increment(1);
                                metrics::gauge!("channel/recv_bytes", "channel" => channel_name).increment(fragment_data.bytes.len() as f64);
                            }
                            trace!(
                                target: "lightyear_debug::transport",
                                kind = "channel_recv_fragment",
                                schedule = "PreUpdate",
                                sample_point = "PreUpdate",
                                entity = ?entity,
                                packet_id = ?header.packet_id,
                                remote_tick = tick.0,
                                channel_id,
                                channel = channel_name,
                                bytes = fragment_data.bytes.len(),
                                "received channel fragment"
                            );
                            transport
                                .receivers
                                .get_mut(&channel_id)
                                .ok_or(PacketError::ChannelNotFound)?
                                .receiver
                                .buffer_recv(ReceiveMessage {
                                    data: fragment_data.into(),
                                    remote_sent_tick: tick,
                                    compression: transport.compression,
                                })?;
                        }
                        // read single message data
                        while cursor.has_remaining() {
                            let channel_id = ChannelId::from_bytes(&mut cursor)?;
                            let channel_name = channel_registry.get_name_from_net_id(channel_id);
                            let num_messages =
                                cursor.read_u8().map_err(SerializationError::from)?;
                            #[cfg(feature = "metrics")]
                            metrics::gauge!("channel/recv_messages", "channel" => channel_name).increment(num_messages as f64);
                            trace!(?channel_id, ?num_messages);
                            trace!(
                                target: "lightyear_debug::transport",
                                kind = "channel_recv_batch",
                                schedule = "PreUpdate",
                                sample_point = "PreUpdate",
                                entity = ?entity,
                                packet_id = ?header.packet_id,
                                remote_tick = tick.0,
                                channel_id,
                                channel = channel_name,
                                num_messages,
                                "received channel message batch"
                            );
                            for _ in 0..num_messages {
                                let single_data = SingleData::from_bytes(&mut cursor)?;
                                #[cfg(feature = "metrics")]
                                metrics::gauge!("channel/recv_bytes", "channel" => channel_name).increment(single_data.bytes.len() as f64);
                                trace!(
                                    target: "lightyear_debug::transport",
                                    kind = "channel_recv_message",
                                    schedule = "PreUpdate",
                                    sample_point = "PreUpdate",
                                    entity = ?entity,
                                    packet_id = ?header.packet_id,
                                    remote_tick = tick.0,
                                    channel_id,
                                    channel = channel_name,
                                    bytes = single_data.bytes.len(),
                                    "received channel message bytes"
                                );
                                transport
                                    .receivers
                                    .get_mut(&channel_id)
                                    .ok_or(PacketError::ChannelNotFound)?
                                    .receiver
                                    .buffer_recv(ReceiveMessage {
                                        data: single_data.into(),
                                        remote_sent_tick: tick,
                                        compression: transport.compression,
                                    })?;
                            }
                        }
                        Ok::<(), TransportError>(())
                    })
                    .inspect_err(|e| {
                        error!("Error processing packet: {e:?}");
                    })
                    .ok();

                // Update the list of messages that have been acked
                transport
                    .packet_manager
                    .header_manager
                    .newly_acked_packets
                    .drain(..)
                    .try_for_each(|(acked_packet, rtt_sample)| {
                        trace!("Acked packet {:?}", acked_packet);
                        trace!(
                            target: "lightyear_debug::transport",
                            kind = "packet_acked",
                            schedule = "PreUpdate",
                            sample_point = "PreUpdate",
                            entity = ?entity,
                            packet_id = ?acked_packet,
                            rtt_sample_ms = rtt_sample.as_secs_f64() * 1000.0,
                            "transport packet acked"
                        );
                        if let Some(message_acks) =
                            transport.packet_to_message_map.remove(&acked_packet)
                        {
                            for (channel_kind, message_ack) in message_acks {
                                let sender_metadata = transport
                                    .senders
                                    .get_mut(&channel_kind)
                                    .ok_or(PacketError::ChannelNotFound)?;

                                sender_metadata.sender.receive_ack(&message_ack);

                                if message_ack.fragment_id.is_none() {
                                    trace!(
                                        "Acked message in packet: channel={:?},message_ack={:?}",
                                        sender_metadata.name, message_ack
                                    );
                                    sender_metadata.message_acks.push(message_ack.message_id);
                                } else if let Entry::Occupied(mut entry) = transport.fragment_acks.entry(message_ack.message_id) {
                                        let num_fragments = entry.get_mut();
                                        *num_fragments -= 1;
                                        if *num_fragments == 0 {
                                            entry.remove();
                                            trace!(
                                                "Acked all fragments in message: channel={:?},message_ack={:?}",
                                                sender_metadata.name, message_ack
                                            );
                                            sender_metadata.message_acks.push(message_ack.message_id);
                                        }
                                    }
                            }
                        }
                        Ok::<(), TransportError>(())
                    })
                    .ok();
            });
    }

    /// Iterates through the `ChannelSenders` on the entity,
    /// Build packets from the messages in the channel,
    /// Upload the packets to the [`Link`]
    fn buffer_send(
        real_time: Res<Time<Real>>,
        timeline: Res<LocalTimeline>,
        mut query: Query<(&mut Link, &mut Transport, Option<&mut HostClient>), With<Linked>>,
        channel_registry: Res<ChannelRegistry>,
    ) {
        #[cfg(feature = "metrics")]
        let _timer = TimerGauge::new("transport/send");
        let tick = timeline.tick();
        query.par_iter_mut().for_each(|(mut link, mut transport, host_client)| {
            // allow split borrows
            let transport = &mut *transport;

            // buffer all new messages in the Sender
            if let Some(mut host_client) = host_client {
                // for a host-client, we write the bytes directly to the HostClient buffer
                transport.recv_channel.try_iter().try_for_each(|(channel_kind, bytes, priority)| {
                    host_client.buffer.push((bytes, channel_kind.0));
                    Ok::<(), TransportError>(())
                }).inspect_err(|e| error!("error buffering host-client message: {e:?}")).ok();
                return
            }
            transport.recv_channel.try_iter().try_for_each(|(channel_kind, bytes, priority)| {
                let sender_metadata = transport.senders.get_mut(&channel_kind).ok_or(TransportError::ChannelNotFound(channel_kind))?;
                trace!(
                    target: "lightyear_debug::transport",
                    kind = "channel_send_buffer",
                    schedule = "PostUpdate",
                    sample_point = "PostUpdate",
                    tick = ?tick,
                    tick_id = u64::from(tick.0),
                    channel = %sender_metadata.name,
                    channel_kind = ?channel_kind,
                    bytes = bytes.len(),
                    priority = priority,
                    "buffered channel message for transport send"
                );
                // TODO: do we need the message_id?
                sender_metadata
                    .sender
                    .buffer_send(bytes, priority, transport.compression);
                Ok::<(), TransportError>(())
            }).inspect_err(|e| error!("error sending message: {e:?}")).ok();

            // Collect cheap snapshots while each channel retains ownership of its pending queues.
            transport.priority_manager.clear();
            {
                let candidates = transport.priority_manager.candidates_mut();
                transport.senders.iter_mut().for_each(|(channel_kind, metadata)| {
                    metadata.sender.collect_send_candidates(
                        *channel_kind,
                        metadata.channel_id,
                        candidates,
                    );
                });
            }
            transport.priority_manager.prioritize(&channel_registry);

            let mut candidate_cursor = crate::packet::packet_builder::CandidateCursor::default();
            let mut total_bytes_sent = 0;
            let mut flush_outcome = SendFlushOutcome::Complete;
            loop {
                let staged = transport.packet_manager.build_next_packet(
                    tick,
                    transport.priority_manager.candidates(),
                    &mut candidate_cursor,
                    transport.compression,
                );
                let mut packet = match staged {
                    Ok(Some(packet)) => packet,
                    Ok(None) => break,
                    Err(error) => {
                        flush_outcome = SendFlushOutcome::StagingFailed;
                        error!(?error, "failed to stage transport packet");
                        break;
                    }
                };
                trace!(packet_id = ?packet.packet_id, num_messages = ?packet.num_messages(), "sending packet");
                let packet_id = packet.packet_id;
                let num_messages = packet.num_messages();
                let packet_len = packet.payload.len();
                let packet_compression = packet.compression;
                if !transport
                    .bandwidth_limiter
                    .consume_packet_quota(packet_len as u32)
                {
                    // Staging has no channel or packet-header side effects. Reliable messages are
                    // retained; each unreliable channel applies its retry-unsent policy when this
                    // flush finishes. Since bandwidth limiting also enables priority ordering,
                    // later candidates are not higher priority than this packet.
                    flush_outcome = SendFlushOutcome::BandwidthLimited;
                    transport.packet_manager.recycle_packet(packet);
                    break;
                }
                if let Some(compression_info) = packet.compression {
                    trace!(
                        original_len = compression_info.original_len,
                        compressed_len = compression_info.compressed_len,
                        "transport packet was compressed by packet builder"
                    );
                }
                trace!(
                    target: "lightyear_debug::transport",
                    kind = "packet_send",
                    schedule = "PostUpdate",
                    sample_point = "PostUpdate",
                    packet_id = ?packet_id,
                    local_tick = tick.0,
                    bytes = packet_len,
                    num_messages,
                    compression_enabled = transport.compression.is_enabled(),
                    compression_algorithm = ?transport.compression.algorithm,
                    packet_compressed = packet_compression.is_some(),
                    compression_original_len = packet_compression.map_or(0, |info| info.original_len),
                    compression_compressed_len = packet_compression.map_or(0, |info| info.compressed_len),
                    "sending transport packet"
                );

                // Acceptance into Link.send is the transactional boundary. Packet ids, retry
                // timestamps, ack maps, and metrics are committed only after this point.
                total_bytes_sent += packet.payload.len() as u32;
                link.send.push(Bytes::from(core::mem::take(&mut packet.payload)));
                transport
                    .packet_manager
                    .header_manager
                    .commit_send_packet(packet_id, real_time.elapsed());

                #[cfg(feature = "metrics")]
                if let Some(compression_info) = packet.compression {
                    metrics::counter!("transport/compression_saved_bytes").increment(
                        (compression_info.original_len - compression_info.compressed_len) as u64,
                    );
                }

                let mut packet_messages = core::mem::take(&mut packet.messages);
                for metadata in packet_messages.drain(..) {
                    let commit = metadata.commit;
                    let sender_metadata = transport
                        .senders
                        .get_mut(&commit.channel_kind)
                        .expect("staged candidate channel must remain registered during flush");
                    sender_metadata
                        .sender
                        .commit_send(commit.key, real_time.elapsed());

                    #[cfg(feature = "metrics")]
                    {
                        let channel_name =
                            channel_registry.get_name_from_net_id(metadata.channel);
                        metrics::gauge!("channel/send_messages", "channel" => channel_name)
                            .increment(1);
                        metrics::gauge!("channel/send_bytes", "channel" => channel_name)
                            .increment(metadata.num_bytes as f64);
                    }

                    let Some(message_id) = metadata.message else {
                        continue;
                    };
                    sender_metadata.messages_sent.push(message_id);
                    if sender_metadata.mode.is_watching_acks() {
                        trace!(
                            "Registering message ack (ChannelId:{:?} {:?}) for packet {:?}",
                            metadata.channel, metadata, packet.packet_id
                        );

                        if let Some(num_fragments) = metadata.num_fragments {
                            transport
                                .fragment_acks
                                .entry(message_id)
                                .or_insert(num_fragments);
                        }
                        transport
                            .packet_to_message_map
                            .entry(packet.packet_id)
                            .or_default()
                            .push((
                                commit.channel_kind,
                                MessageAck {
                                    message_id,
                                    fragment_id: metadata.fragment_index,
                                },
                            ));
                        trace!(?transport.packet_to_message_map, "packet to message");
                    }
                }
                transport
                    .packet_manager
                    .recycle_message_metadata_list(packet_messages);
            }
            transport
                .senders
                .values_mut()
                .for_each(|metadata| metadata.sender.finish_send(flush_outcome));
            transport.priority_manager.clear();
            if total_bytes_sent > 0 {
                trace!(
                    target: "lightyear_debug::transport",
                    kind = "send_flush",
                    schedule = "PostUpdate",
                    sample_point = "PostUpdate",
                    local_tick = tick.0,
                    send_bytes = total_bytes_sent,
                    "flushed transport packets to link"
                );
            }

            #[cfg(feature = "metrics")]
            metrics::gauge!("transport/send_bytes").increment(total_bytes_sent as f64);
        });
    }

    /// On disconnection, reset the Transport to its original state.
    #[cfg(any(feature = "client", feature = "server"))]
    fn handle_disconnection(
        trigger: On<Add, Disconnected>,
        mut query: Query<&mut Transport>,
        registry: Res<ChannelRegistry>,
    ) {
        if let Ok(mut transport) = query.get_mut(trigger.entity) {
            transport.reset(&registry);
        }
    }
}

impl Plugin for TransportPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<LinkPlugin>() {
            app.add_plugins(LinkPlugin);
        }
        #[cfg(any(feature = "client", feature = "server"))]
        app.add_observer(Self::handle_disconnection);
    }

    fn finish(&self, app: &mut App) {
        if !app.world().contains_resource::<ChannelRegistry>() {
            warn!("TransportPlugin: ChannelRegistry not found, adding it");
            app.world_mut().init_resource::<ChannelRegistry>();
        }
        app.configure_sets(
            PreUpdate,
            TransportSystems::Receive.after(LinkSystems::Receive),
        );
        app.configure_sets(PostUpdate, TransportSystems::Send.before(LinkSystems::Send));
        app.add_systems(
            PreUpdate,
            Self::buffer_receive.in_set(TransportSystems::Receive),
        );
        app.add_systems(PostUpdate, Self::buffer_send.in_set(TransportSystems::Send));
    }
}

#[cfg(feature = "test_utils")]
pub struct TestChannel;

#[cfg(feature = "test_utils")]
pub struct TestTransportPlugin;

#[cfg(feature = "test_utils")]
impl Plugin for TestTransportPlugin {
    fn build(&self, app: &mut App) {
        // add all channels before adding the TransportPlugin
        app.init_resource::<ChannelRegistry>();
        app.add_channel::<TestChannel>(ChannelSettings {
            mode: ChannelMode::UnorderedUnreliable,
            ..default()
        });
        // add required resources
        app.init_resource::<Time<Real>>();
        // add the TransportPlugin
        app.add_plugins(TransportPlugin);
    }
}

#[cfg(test)]
mod tests {
    use alloc::{vec, vec::Vec};

    use super::*;
    use crate::channel::builder::{ChannelMode, ChannelSettings};
    use crate::channel::registry::ChannelKind;
    use crate::packet::priority_manager::PriorityConfig;
    use bevy_ecs::system::RunSystemOnce;

    struct RetryChannel;
    struct DiscardChannel;

    fn spawn_transport<C: crate::channel::Channel>(
        world: &mut World,
        settings: ChannelSettings,
        channel_kind: ChannelKind,
        channel_id: ChannelId,
    ) -> Entity {
        let mut transport = Transport::new(PriorityConfig::new(1));
        transport.add_sender::<C>((&settings).into(), settings.mode, channel_id);
        for value in [1, 2] {
            transport
                .send_mut_erased(channel_kind, Bytes::from(vec![value; 1000]), 1.0)
                .unwrap();
        }
        world.spawn((Link::default(), Linked, transport)).id()
    }

    fn pending_candidates<C: crate::channel::Channel>(world: &mut World, entity: Entity) -> usize {
        let mut entity = world.entity_mut(entity);
        let mut transport = entity.get_mut::<Transport>().unwrap();
        let channel_kind = ChannelKind::of::<C>();
        let mut candidates = Vec::new();
        let metadata = transport.senders.get_mut(&channel_kind).unwrap();
        metadata
            .sender
            .collect_send_candidates(channel_kind, metadata.channel_id, &mut candidates);
        candidates.len()
    }

    #[test]
    fn bandwidth_limited_flush_applies_each_channels_retry_policy() {
        let retry_settings = ChannelSettings {
            mode: ChannelMode::UnorderedUnreliable,
            retry_unsent_messages: true,
            ..Default::default()
        };
        let discard_settings = ChannelSettings {
            retry_unsent_messages: false,
            ..retry_settings
        };
        let mut registry = ChannelRegistry::default();
        let (retry_kind, retry_id) = registry.add_channel::<RetryChannel>(retry_settings);
        let (discard_kind, discard_id) = registry.add_channel::<DiscardChannel>(discard_settings);

        let mut world = World::new();
        world.insert_resource(registry);
        world.init_resource::<Time<Real>>();
        world.init_resource::<LocalTimeline>();
        let retry_entity =
            spawn_transport::<RetryChannel>(&mut world, retry_settings, retry_kind, retry_id);
        let discard_entity = spawn_transport::<DiscardChannel>(
            &mut world,
            discard_settings,
            discard_kind,
            discard_id,
        );

        world.run_system_once(TransportPlugin::buffer_send).unwrap();

        assert_eq!(world.get::<Link>(retry_entity).unwrap().send.len(), 1);
        assert_eq!(world.get::<Link>(discard_entity).unwrap().send.len(), 1);
        assert_eq!(
            pending_candidates::<RetryChannel>(&mut world, retry_entity),
            1
        );
        assert_eq!(
            pending_candidates::<DiscardChannel>(&mut world, discard_entity),
            0
        );
    }
}
