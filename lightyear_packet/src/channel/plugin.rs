use crate::channel::builder::Transport;
use crate::channel::receivers::ChannelReceive;
use crate::channel::registry::{ChannelId, ChannelRegistry};
use crate::packet::error::PacketError;
use crate::packet::header::PacketHeader;
use crate::packet::message::{FragmentData, ReceiveMessage, SendMessage, SingleData};
use bevy::app::App;
use bevy::ecs::system::{ParamBuilder, QueryParamBuilder};
use bevy::ecs::world::FilteredEntityMut;
use bevy::prelude::*;
use bytes::Bytes;
use lightyear_core::network::NetId;
use lightyear_core::tick::TickManager;
use lightyear_link::{Link, LinkSet};
use lightyear_serde::reader::{ReadInteger, Reader};
use lightyear_serde::{SerializationError, ToBytes};
use std::collections::VecDeque;
use tracing::{error, trace};


#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone, Copy)]
pub enum TransportSet {
    // PRE UPDATE
    /// Receive messages from the Link and buffer them into the ChannelReceivers
    Receive,

    // PostUpdate
    /// Flush the messages buffered in the ChannelSenders to the Link
    Send,
}

pub struct ChannelsPlugin;

impl ChannelsPlugin {

    /// Receives packets from the [`Link`],
    /// Depending on the [`ChannelId`], buffer the messages in the packet
    /// in the appropriate [`ChannelReceiver`]
    fn buffer_receive(
        mut link_query: Query<(Entity, &mut Link, &mut Transport)>,
        mut sender_query: Query<FilteredEntityMut>
    ) -> Result {
        link_query.iter_mut().try_for_each(|(entity, mut link, mut transport)| {
            link.recv.drain(..).try_for_each(|packet| {
                let mut cursor = Reader::from(packet);

                // Parse the packet
                let header = PacketHeader::from_bytes(&mut cursor)?;
                let tick = header.tick;

                // Update the packet acks
                let acked_packets = transport
                    .packet_manager
                    .header_manager
                    .process_recv_packet_header(&header);

                // Update the list of messages that have been acked
                for acked_packet in acked_packets {
                    trace!("Acked packet {:?}", acked_packet);
                    if let Some(message_acks) = transport.packet_to_message_ack_map.remove(&acked_packet) {
                        for (channel_kind, message_ack) in message_acks {
                            let sender_metadata = transport.senders.get_mut(&channel_kind).ok_or(PacketError::ChannelNotFound)?;
                            trace!(
                                "Acked message in packet: channel={:?},message_ack={:?}",
                                sender_metadata.name,
                                message_ack
                            );
                            if let Ok(mut f) = sender_query.get_mut(entity) {
                                if let Some(sender) = f.get_mut_by_id(sender_metadata.sender_id) {
                                    (sender_metadata.receive_ack)(sender, message_ack);
                                }
                            }
                        }
                    }
                }

                // Parse the payload into messages, put them in the internal buffers for each channel
                // we read directly from the packet and don't create intermediary datastructures to avoid allocations
                // TODO: maybe do this in a helper function?
                if header.get_packet_type() == crate::packet::packet_type::PacketType::DataFragment {
                    // read the fragment data
                    let channel_id = ChannelId::from_bytes(&mut cursor)?;
                    let fragment_data = FragmentData::from_bytes(&mut cursor)?;
                    transport.receivers.get_mut(&channel_id).ok_or(PacketError::ChannelNotFound)?
                        .buffer_recv(ReceiveMessage {
                            data: fragment_data.into(),
                            remote_sent_tick: tick,
                        })?;
                }
                // read single message data
                while cursor.has_remaining() {
                    let channel_id = ChannelId::from_bytes(&mut cursor)?;
                    let num_messages = cursor.read_u8().map_err(SerializationError::from)?;
                    trace!(?channel_id, ?num_messages);
                    for _ in 0..num_messages {
                        let single_data = SingleData::from_bytes(&mut cursor)?;
                        transport.receivers.get_mut(&channel_id).ok_or(PacketError::ChannelNotFound)?
                            .buffer_recv(ReceiveMessage {
                                data: single_data.into(),
                                remote_sent_tick: tick,
                            })?;
                    }
                }
                Ok(())
            })
        })
    }

    /// Iterates through the ChannelSenders on the entity,
    /// Build packets from the messages in the channel,
    /// Upload the packets to the link
    fn buffer_send(
        mut link_query: Query<(Entity, &mut Link, &mut Transport)>,
        mut sender_query: Query<FilteredEntityMut>,
        channel_registry: Res<ChannelRegistry>,
        tick_manager: Res<TickManager>,
    ) -> Result {
        let tick = tick_manager.tick();
        // TODO: add parallelism
        link_query.iter_mut().try_for_each(|(entity, mut link, mut transport)| {
            let mut transport = &mut *transport;
            // flush messages from the ChannelSender to the actual sender
            transport.senders.values().for_each(|sender_metadata| {
                if let Ok(mut f) = sender_query.get_mut(entity) {
                    if let Some(sender) = f.get_mut_by_id(sender_metadata.sender_id) {
                        (sender_metadata.flush)(&mut transport.priority_manager, sender);
                    }
                }
            });

            // get the list of messages that we can send according to the bandwidth limiter
            let (single_data, fragment_data, num_bytes_added_to_limiter) = transport
                .priority_manager
                .priority_filter(&channel_registry, tick);

            // build actual packets from these messages
            // TODO: swap to try_for_each when available
            let packets =
                transport.packet_manager
                    .build_packets(tick, single_data, fragment_data)?;

            let mut total_bytes_sent = 0;
            for mut packet in packets {
                trace!(packet_id = ?packet.packet_id, num_messages = ?packet.num_messages(), "sending packet");
                // TODO: should we update this to include fragment info as well?
                // Step 2. Update the packet_to_message_id_map (only for channels that care about acks)
                core::mem::take(&mut packet.message_acks)
                    .into_iter()
                    .try_for_each(|(channel_id, message_ack)| {
                        // TODO: get channel_settings from ECS
                        let channel_kind = channel_registry
                            .get_kind_from_net_id(channel_id)
                            .ok_or(PacketError::ChannelNotFound)?;
                        let sender_metadata = transport.senders
                            .get(channel_kind)
                            .ok_or(PacketError::ChannelNotFound)?;
                        if sender_metadata.mode.is_watching_acks() {
                            trace!(
                                "Registering message ack (ChannelId:{:?} {:?}) for packet {:?}",
                                channel_id,
                                message_ack,
                                packet.packet_id
                            );
                            transport.packet_to_message_ack_map
                                .entry(packet.packet_id)
                                .or_default()
                                .push((*channel_kind, message_ack));
                        }
                        Ok::<(), PacketError>(())
                    })?;

                // Upload the packets to the link
                total_bytes_sent += packet.payload.len() as u32;
                link.send.push(Bytes::from(packet.payload));
            }

            // adjust the real amount of bytes that we sent through the limiter (to account for the actual packet size)
            if transport.priority_manager.config.enabled {
                if let Ok(remaining_bytes_to_add) =
                    (total_bytes_sent - num_bytes_added_to_limiter).try_into()
                {
                    let _ = transport
                        .priority_manager
                        .limiter
                        .check_n(remaining_bytes_to_add);
                }
            }
            Ok(())
        })
    }
}


impl Plugin for ChannelsPlugin {
    fn build(&self, app: &mut App) {

        // temporarily remove the ChannelRegistry from the app to enable split borrows
        let mut channel_registry = app.world_mut().remove_resource::<ChannelRegistry>().unwrap();

        let buffer_receive = (
            ParamBuilder,
            QueryParamBuilder::new(|builder| {
                builder.optional(|b| {
                    channel_registry.sender_ids.iter().for_each(|sender_id| {
                        b.mut_id(*sender_id);
                    });
                });
            }),
        )
            .build_state(app.world_mut())
            .build_system(Self::buffer_receive);

        let buffer_send = (
            ParamBuilder,
            QueryParamBuilder::new(|builder| {
                builder.optional(|b| {
                    channel_registry.sender_ids.iter().for_each(|sender_id| {
                        b.mut_id(*sender_id);
                    });
                });
            }),
            ParamBuilder,
            ParamBuilder
        )
            .build_state(app.world_mut())
            .build_system(Self::buffer_send);

        app.configure_sets(PreUpdate, TransportSet::Receive.after(LinkSet::Receive));
        app.configure_sets(PostUpdate, TransportSet::Send.after(LinkSet::Send));
        app.add_systems(PreUpdate, buffer_receive.in_set(TransportSet::Receive));
        app.add_systems(PostUpdate, buffer_send.in_set(TransportSet::Send));

        // re-insert the channel registry
        app.world_mut().insert_resource(channel_registry);
    }
}


