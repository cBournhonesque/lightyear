use std::collections::HashMap;

use anyhow::{anyhow, Context};
use bitcode::read::Read;

use crate::channel::channel::ChannelContainer;
use crate::channel::receivers::ChannelReceive;
use crate::channel::senders::ChannelSend;
use crate::packet::message::MessageContainer;
use crate::packet::packet::Packet;
use crate::packet::packet_manager::PacketManager;
use crate::packet::wrapping_id::{MessageId, PacketId};
use crate::protocol::Protocol;
use crate::serialize::reader::ReadBuffer;
use crate::transport::{PacketReader, PacketReceiver, PacketSender, Transport};
use crate::{BitSerializable, Channel, ChannelKind, ChannelRegistry, WriteBuffer};

/// Wrapper to: send/receive messages via channels to a remote address
/// By splitting the data into packets and sending them through a given transport
pub struct MessageManager<M: BitSerializable> {
    /// Handles sending/receiving packets (including acks)
    packet_manager: PacketManager<M>,
    // TODO: add ordering of channels per priority
    channels: HashMap<ChannelKind, ChannelContainer<M>>,
    // TODO: can use Vec<ChannelKind, Vec<MessageId>> to be more efficient?
    /// Map to keep track of which messages have been sent in which packets, so that
    /// reliable senders can stop trying to send a message that has already been received
    packet_to_message_id_map: HashMap<PacketId, HashMap<ChannelKind, Vec<MessageId>>>,
}

impl<M: BitSerializable> MessageManager<M> {
    pub fn new(channel_registry: &ChannelRegistry) -> Self {
        Self {
            packet_manager: PacketManager::new(channel_registry.kind_map()),
            channels: channel_registry.channels(),
            packet_to_message_id_map: HashMap::new(),
        }
    }

    /// Buffer a message to be sent on this connection
    pub fn buffer_send<C: Channel>(&mut self, message: M) -> anyhow::Result<()> {
        let channel_kind = ChannelKind::of::<C>();
        let mut channel = self
            .channels
            .get_mut(&channel_kind)
            .context("Channel not found")?;
        channel.sender.buffer_send(MessageContainer::new(message));
        Ok(())
    }

    /// Prepare buckets from the internal send buffers, and return the bytes to send
    // pub fn send_packets(&mut self, io: &mut impl PacketSender) -> anyhow::Result<()> {
    pub fn send_packets(&mut self) -> anyhow::Result<Vec<impl WriteBuffer>> {
        // Step 1. Get the list of packets to send from all channels
        // for each channel, prepare packets using the buffered messages that are ready to be sent
        for (channel_kind, channel) in self.channels.iter_mut() {
            channel.sender.collect_messages_to_send();
            if channel.sender.has_messages_to_send() {
                // start a new channel in the current packet.
                // If there's not enough space, start writing in a new packet
                if !self.packet_manager.can_add_channel(*channel_kind)? {
                    self.packet_manager.build_new_packet();
                    // can add channel starts writing a new packet if needed
                    let added_new_channel = self.packet_manager.can_add_channel(*channel_kind)?;
                    debug_assert!(added_new_channel);
                }
                channel.sender.send_packet(&mut self.packet_manager);
            }
        }

        let packets = self.packet_manager.flush_packets();

        // TODO: might need to split into single packets?
        let mut bytes = Vec::new();
        for packet in packets {
            // Step 2. Send the packets over the network
            let payload = self.packet_manager.encode_packet(&packet)?;
            bytes.push(payload);
            // io.send(payload, &self.remote_addr)?;

            // Step 3. Update the packet_to_message_id_map (only for reliable channels)
            packet
                .message_ids()
                .iter()
                .try_for_each(|(channel_id, message_ids)| {
                    let channel_kind = self
                        .packet_manager
                        .channel_kind_map
                        .kind(*channel_id)
                        .context("cannot find channel kind")?;
                    let channel = self
                        .channels
                        .get(channel_kind)
                        .context("Channel not found")?;
                    let packet_id = packet.header().packet_id;
                    if channel.setting.mode.is_reliable() {
                        self.packet_to_message_id_map
                            .entry(packet_id)
                            .or_default()
                            .entry(*channel_kind)
                            .or_default()
                            .append(&mut message_ids.clone());
                    }
                    Ok::<(), anyhow::Error>(())
                })?;
        }

        Ok(bytes)
    }

    // /// Listen for packets on the transport and buffer them
    // ///
    // /// Return when there are no more packets to receive on the transport
    // pub fn recv_packets<T: ReadBuffer>(
    //     &mut self,
    //     io: &mut impl PacketReader,
    // ) -> anyhow::Result<()> {
    //     loop {
    //         match io.read::<T>()? {
    //             None => break,
    //             Some((mut reader, addr)) => {
    //                 // this copies the data into the buffer, so we can read efficiently from it
    //                 // we can now re-use the transport's buffer.
    //                 // maybe it would be safer to provide a buffer for the transport to use?
    //                 if addr != self.remote_addr {
    //                     bail!("received packet from unknown address");
    //                 }
    //                 self.recv_packet(&mut reader)?;
    //                 continue;
    //             }
    //         }
    //     }
    //     Ok(())
    // }

    /// Process packet received over the network as raw bytes
    /// Update the acks, and put the messages from the packets in internal buffers
    pub fn recv_packet(&mut self, reader: &mut impl ReadBuffer) -> anyhow::Result<()> {
        // Step 1. Parse the packet
        let packet: Packet<M> = self.packet_manager.decode_packet(reader)?;
        let packet = match packet {
            Packet::Single(single_packet) => single_packet,
            Packet::Fragmented(_) => unimplemented!(),
        };

        // TODO: an option is to have an async task that is on the receiving side of the
        //  cross-beam channel which tell which packets have been received

        // Step 2. Update the packet acks (which packets have we received, and which of our packets
        // have been acked)
        let acked_packets = self
            .packet_manager
            .header_manager
            .process_recv_packet_header(&packet.header);

        // Step 3. Update the list of messages that have been acked
        for acked_packet in acked_packets {
            if let Some(message_map) = self.packet_to_message_id_map.remove(&acked_packet) {
                for (channel_kind, message_ids) in message_map {
                    let channel = self
                        .channels
                        .get_mut(&channel_kind)
                        .context("Channel not found")?;
                    for message_id in message_ids {
                        channel.sender.notify_message_delivered(&message_id);
                    }
                }
            }
        }

        // Step 4. Put the messages from the packet in the internal buffers for each channel
        for (channel_net_id, messages) in packet.data {
            let channel_kind = self
                .packet_manager
                .channel_kind_map
                .kind(channel_net_id)
                .context(format!(
                    "Could not recognize net_id {} as a channel",
                    channel_net_id
                ))?;
            let channel = self
                .channels
                .get_mut(channel_kind)
                .ok_or_else(|| anyhow!("Channel not found"))?;
            for message in messages {
                channel.receiver.buffer_recv(message)?;
            }
        }

        Ok(())
    }

    /// Read all the messages in the internal buffers that are ready to be processed
    // TODO: this is where naia converts the messages to events and pushes them to an event queue
    //  let be conservative and just return the messages right now. We could switch to an iterator
    pub fn read_messages(&mut self) -> HashMap<ChannelKind, Vec<M>> {
        let mut map = HashMap::new();
        for (channel_kind, channel) in self.channels.iter_mut() {
            let mut messages = vec![];
            while let Some(message) = channel.receiver.read_message() {
                messages.push(message.inner());
            }
            if !messages.is_empty() {
                map.insert(*channel_kind, messages);
            }
        }
        map
    }
}

// TODO: have a way to update the channels about the messages that have been acked

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::str::FromStr;

    use serde::{Deserialize, Serialize};

    use lightyear_derive::ChannelInternal;

    use crate::channel::channel::ReliableSettings;
    use crate::packet::wrapping_id::{MessageId, PacketId};
    use crate::transport::Transport;
    use crate::{
        ChannelDirection, ChannelKind, ChannelMode, ChannelRegistry, ChannelSettings,
        MessageContainer, MessageManager, Protocol, ReadBuffer, ReadWordBuffer, WriteBuffer,
    };

    // Messages
    #[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
    pub struct Message1(pub u8);

    #[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
    pub struct Message2(pub u32);

    #[derive(Debug, PartialEq, Clone, Serialize, Deserialize)]
    pub enum MyMessageProtocol {
        Message1(Message1),
        Message2(Message2),
    }

    // Channels
    #[derive(ChannelInternal)]
    struct Channel1;

    #[derive(ChannelInternal)]
    struct Channel2;

    #[test]
    /// We want to test that we can send/receive messages over a connection
    fn test_connection() -> Result<(), anyhow::Error> {
        let mut channel_registry = ChannelRegistry::new();
        channel_registry.add::<Channel1>(ChannelSettings {
            mode: ChannelMode::OrderedReliable(ReliableSettings::default()),
            direction: ChannelDirection::Bidirectional,
        });
        channel_registry.add::<Channel2>(ChannelSettings {
            mode: ChannelMode::UnorderedUnreliable,
            direction: ChannelDirection::Bidirectional,
        });

        // Create connections
        let mut client_connection = MessageManager::<MyMessageProtocol>::new(&channel_registry);
        let mut server_connection = MessageManager::<MyMessageProtocol>::new(&channel_registry);

        // client: buffer send messages, and then send
        let with_id = |message: &MessageContainer<_>, id| {
            let mut m = message.clone();
            m.set_id(MessageId(id));
            m
        };

        let mut message = MessageContainer::new(MyMessageProtocol::Message1(Message1(1)));
        let channel_kind_1 = ChannelKind::of::<Channel1>();
        let channel_kind_2 = ChannelKind::of::<Channel2>();
        client_connection.buffer_send::<Channel1>(message.clone())?;
        client_connection.buffer_send::<Channel2>(message.clone())?;
        let mut packet_bytes = client_connection.send_packets()?;
        assert_eq!(
            client_connection.packet_to_message_id_map,
            HashMap::from([(
                PacketId(0),
                HashMap::from([(channel_kind_1.clone(), vec![MessageId(0)])])
            )])
        );

        // server: receive bytes from the sent messages, then process them into messages
        for mut packet_byte in packet_bytes.iter_mut() {
            server_connection
                .recv_packet(&mut ReadWordBuffer::start_read(&packet_byte.finish_write()))?;
        }
        let mut data = server_connection.read_messages();
        assert_eq!(
            data.get(&channel_kind_1).unwrap(),
            &vec![with_id(&message, 0)]
        );
        assert_eq!(data.get(&channel_kind_2).unwrap(), &vec![message.clone()]);

        // Confirm what happens if we try to receive but there is nothing on the io
        data = server_connection.read_messages();
        assert!(data.is_empty());

        // Check the state of the packet headers
        assert_eq!(
            client_connection
                .packet_manager
                .header_manager
                .next_packet_id(),
            PacketId(1)
        );
        assert!(client_connection
            .packet_manager
            .header_manager
            .sent_packets_not_acked()
            .contains(&PacketId(0)));

        // Server sends back a message
        server_connection.buffer_send::<Channel1>(message.clone())?;
        let mut packet_bytes = server_connection.send_packets()?;

        // On client side: keep looping to receive bytes on the network, then process them into messages
        for mut packet_byte in packet_bytes.iter_mut() {
            client_connection
                .recv_packet(&mut ReadWordBuffer::start_read(&packet_byte.finish_write()))?;
        }

        // Check that reliability works correctly
        assert_eq!(client_connection.packet_to_message_id_map.len(), 0);
        // TODO: check that client_channel_1's sender's unacked messages is empty
        // let client_channel_1 = client_connection.channels.get(&channel_kind_1).unwrap();
        // assert_eq!(client_channel_1.sender.)
        Ok(())
    }
}
