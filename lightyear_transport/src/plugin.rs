use crate::channel::builder::Transport;
use crate::channel::receivers::ChannelReceive;
use crate::channel::registry::{ChannelId, ChannelRegistry};
use crate::channel::senders::ChannelSend;
use crate::error::TransportError;
use crate::packet::error::PacketError;
use crate::packet::header::PacketHeader;
use crate::packet::message::{FragmentData, MessageAck, ReceiveMessage, SingleData};
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
use lightyear_connection::host::HostClient;
#[cfg(any(feature = "client", feature = "server"))]
use lightyear_connection::prelude::Disconnected;
use lightyear_core::prelude::{LocalTimeline, NetworkTimeline};
use lightyear_core::tick::Tick;
use lightyear_link::{Link, LinkPlugin, LinkSystems, Linked};
use lightyear_serde::reader::{ReadInteger, Reader};
use lightyear_serde::{SerializationError, ToBytes};
#[cfg(feature = "metrics")]
use lightyear_utils::metrics::TimerGauge;
#[allow(unused_imports)]
use tracing::{error, info, trace, warn};

#[deprecated(since = "0.25", note = "Use TransportSystems instead")]
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

pub struct TransportPlugin;

impl TransportPlugin {
    /// Receives packets from the [`Link`],
    /// Depending on the [`ChannelId`], buffer the messages in the packet
    /// in the appropriate channel receiver
    fn buffer_receive(
        time: Res<Time<Real>>,
        par_commands: ParallelCommands,
        #[cfg(feature = "metrics")] channel_registry: Res<ChannelRegistry>,
        mut query: Query<(Entity, &mut Link, &mut Transport), (With<Linked>, Without<HostClient>)>,
    ) {
        #[cfg(feature = "metrics")]
        let _timer = TimerGauge::new("transport/recv");

        query
            .par_iter_mut()
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
                    .update(time.delta(), &link.stats);
                transport
                    .packet_manager
                    .header_manager
                    .lost_packets
                    .drain(..)
                    .try_for_each(|lost_packet| {
                        #[cfg(feature = "metrics")]
                        metrics::counter!("transport/packets_lost").increment(1);
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
                        #[cfg(feature = "metrics")]
                        metrics::gauge!("transport/recv_bytes").increment(packet.len() as f64);

                        let mut cursor = Reader::from(packet);

                        // Parse the packet
                        let header = PacketHeader::from_bytes(&mut cursor)?;
                        let tick = header.tick;

                        // TODO: maybe switch to event buffer instead of triggers?
                        par_commands.command_scope(|mut commands| {
                            commands.trigger(PacketReceived { entity, remote_tick: tick });
                        });

                        // Update the packet acks
                        transport
                            .packet_manager
                            .header_manager
                            .process_recv_packet_header(&header);



                        // Parse the payload into messages, put them in the internal buffers for each channel
                        // we read directly from the packet and don't create intermediary datastructures to avoid allocations
                        // TODO: maybe do this in a helper function?
                        if header.get_packet_type()
                            == crate::packet::packet_type::PacketType::DataFragment
                        {
                            // read the fragment data
                            let channel_id = ChannelId::from_bytes(&mut cursor)?;
                            let fragment_data = FragmentData::from_bytes(&mut cursor)?;
                            #[cfg(feature = "metrics")]
                            {
                                let channel_name = channel_registry.get_name_from_net_id(channel_id);
                                metrics::gauge!("channel/recv_messages", "channel" => channel_name).increment(1);
                                metrics::gauge!("channel/recv_bytes", "channel" => channel_name).increment(fragment_data.bytes.len() as f64);
                            }
                            transport
                                .receivers
                                .get_mut(&channel_id)
                                .ok_or(PacketError::ChannelNotFound)?
                                .receiver
                                .buffer_recv(ReceiveMessage {
                                    data: fragment_data.into(),
                                    remote_sent_tick: tick,
                                })?;
                        }
                        // read single message data
                        while cursor.has_remaining() {
                            let channel_id = ChannelId::from_bytes(&mut cursor)?;
                            #[cfg(feature = "metrics")]
                            let channel_name = channel_registry.get_name_from_net_id(channel_id);
                            let num_messages =
                                cursor.read_u8().map_err(SerializationError::from)?;
                            #[cfg(feature = "metrics")]
                            metrics::gauge!("channel/recv_messages", "channel" => channel_name).increment(num_messages as f64);
                            trace!(?channel_id, ?num_messages);
                            for _ in 0..num_messages {
                                let single_data = SingleData::from_bytes(&mut cursor)?;
                                #[cfg(feature = "metrics")]
                                metrics::gauge!("channel/recv_bytes", "channel" => channel_name).increment(single_data.bytes.len() as f64);
                                transport
                                    .receivers
                                    .get_mut(&channel_id)
                                    .ok_or(PacketError::ChannelNotFound)?
                                    .receiver
                                    .buffer_recv(ReceiveMessage {
                                        data: single_data.into(),
                                        remote_sent_tick: tick,
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
                    .try_for_each(|acked_packet| {
                        trace!("Acked packet {:?}", acked_packet);
                        if let Some(message_acks) =
                            transport.packet_to_message_map.remove(&acked_packet)
                        {
                            for (channel_kind, message_ack) in message_acks {
                                let sender_metadata = transport
                                    .senders
                                    .get_mut(&channel_kind)
                                    .ok_or(PacketError::ChannelNotFound)?;

                                if message_ack.fragment_id.is_none() {
                                    trace!(
                                        "Acked message in packet: channel={:?},message_ack={:?}",
                                        sender_metadata.name, message_ack
                                    );
                                    sender_metadata.message_acks.push(message_ack.message_id);
                                    sender_metadata.sender.receive_ack(&message_ack);
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
                                            sender_metadata.sender.receive_ack(&message_ack);
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
        mut query: Query<
            (
                &mut Link,
                &mut Transport,
                &LocalTimeline,
                Option<&mut HostClient>,
            ),
            With<Linked>,
        >,
        channel_registry: Res<ChannelRegistry>,
    ) {
        #[cfg(feature = "metrics")]
        let _timer = TimerGauge::new("transport/send");

        query.par_iter_mut().for_each(|(mut link, mut transport, timeline, host_client)| {
            let tick = timeline.tick();
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
                // TODO: do we need the message_id?
                sender_metadata.sender.buffer_send(bytes, priority);
                Ok::<(), TransportError>(())
            }).inspect_err(|e| error!("error sending message: {e:?}")).ok();

            // flush messages from the Sender to the priority manager
            transport.senders.values_mut().for_each(|sender_metadata| {
                let channel_id = sender_metadata.channel_id;
                let sender = &mut sender_metadata.sender;
                let (single_data, fragment_data) = sender.send_packet();
                if !single_data.is_empty() || !fragment_data.is_empty() {
                    trace!(?channel_id, "send message with channel_id");
                    transport.priority_manager.buffer_messages(channel_id, single_data, fragment_data);
                }
            });

            // get the list of messages that we can send according to the bandwidth limiter
            let (single_data, fragment_data, num_bytes_added_to_limiter) = transport
                .priority_manager
                .priority_filter(&channel_registry, &mut transport.senders);

            // build actual packets from these messages
            // TODO: swap to try_for_each when available
            let Ok(packets) =
                transport.packet_manager
                    .build_packets(real_time.elapsed(), tick, single_data, fragment_data) else {
                error!("Failed to build packets");
                return
            };

            let mut total_bytes_sent = 0;
            for mut packet in packets {
                trace!(packet_id = ?packet.packet_id, num_messages = ?packet.num_messages(), "sending packet");

                // TODO: should we update this to include fragment info as well?
                // Update the packet_to_message_id_map (only for channels that care about acks)
                core::mem::take(&mut packet.messages)
                    .into_iter()
                    .try_for_each(|metadata| {
                        let channel_id = metadata.channel;
                        let channel_kind = channel_registry
                            .get_kind_from_net_id(channel_id)
                            .ok_or(PacketError::ChannelNotFound)?;
                        let sender_metadata = transport.senders
                            .get_mut(channel_kind)
                            .ok_or(PacketError::ChannelNotFound)?;

                        // note: cannot compute send metrics here because this is just for messages
                        //   that have a message id
                        if sender_metadata.mode.is_watching_acks() {
                            trace!(
                                "Registering message ack (ChannelId:{:?} {:?}) for packet {:?}",
                                channel_id,
                                metadata,
                                packet.packet_id
                            );

                            if let Some(num_fragments) = metadata.num_fragments {
                                transport.fragment_acks.insert(metadata.message, num_fragments);
                            }
                            transport.packet_to_message_map
                                .entry(packet.packet_id)
                                // we could have some old data from wrapped PacketIds, so we start by clearing
                                .and_modify(|v| v.clear())
                                .or_default()
                                .push((*channel_kind, MessageAck {
                                    message_id: metadata.message,
                                    fragment_id: metadata.fragment_index,
                                }));

                        }
                        Ok::<(), PacketError>(())
                    }).inspect_err(|e| error!("Error updating packet to message ack: {e:?}")).ok();

                // Upload the packets to the link
                total_bytes_sent += packet.payload.len() as u32;
                link.send.push(Bytes::from(packet.payload));
            }

            #[cfg(feature = "metrics")]
            metrics::gauge!("transport/send_bytes").increment(total_bytes_sent as f64);

            // adjust the real amount of bytes that we sent through the limiter (to account for the actual packet size)
            if transport.priority_manager.config.enabled
                && let Ok(remaining_bytes_to_add) =
                    (total_bytes_sent - num_bytes_added_to_limiter).try_into()
                {
                    let _ = transport
                        .priority_manager
                        .limiter
                        .check_n(remaining_bytes_to_add);
            }
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
        app.configure_sets(PreUpdate, TransportSystems::Receive.after(LinkSystems::Receive));
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
pub mod tests {
    use super::*;

    use alloc::vec;
    use bevy_app::App;

    /// Check that we can buffer Bytes to a ChannelSender and a packet will get added to the Link
    /// Check that if we put that packet on the receive side of the Link, the Transport will process
    /// them through a ChannelReceiver and we get the same bytes
    #[test]
    #[ignore = "Broken on main"]
    fn test_plugin() {
        let mut app = App::new();
        app.add_plugins(TestTransportPlugin);

        let registry = app.world().resource::<ChannelRegistry>();
        let channel_id = *registry
            .get_net_from_kind(&crate::channel::ChannelKind::of::<TestChannel>())
            .unwrap();
        let mut transport = Transport::default();
        transport.add_sender_from_registry::<TestChannel>(registry);
        transport.add_receiver_from_registry::<TestChannel>(registry);
        let entity_mut = app.world_mut().spawn((Link::default(), transport));
        let entity = entity_mut.id();

        // send bytes
        let send_bytes = Bytes::from(vec![1, 2, 3]);
        entity_mut
            .get::<Transport>()
            .unwrap()
            .send::<TestChannel>(send_bytes.clone())
            .unwrap();
        app.update();
        // check that the send-payload was added to the link
        assert_eq!(
            &app.world_mut()
                .entity(entity)
                .get::<Link>()
                .unwrap()
                .send
                .len(),
            &1
        );

        // transfer that payload to the recv side of the link
        let payload = app
            .world_mut()
            .entity_mut(entity)
            .get_mut::<Link>()
            .unwrap()
            .send
            .pop()
            .unwrap();
        app.world_mut()
            .entity_mut(entity)
            .get_mut::<Link>()
            .unwrap()
            .recv
            .push_raw(payload);

        app.update();
        // check that the bytes are received in the channel
        let (_, recv_bytes, _) = app
            .world_mut()
            .entity_mut(entity)
            .get_mut::<Transport>()
            .unwrap()
            .receivers
            .get_mut(&channel_id)
            .unwrap()
            .receiver
            .read_message()
            .expect("expected to receive message");
        assert_eq!(recv_bytes, send_bytes);
    }
}
