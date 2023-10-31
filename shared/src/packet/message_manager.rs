use std::collections::{BTreeMap, HashMap, VecDeque};
use std::marker::PhantomData;

use anyhow::{anyhow, Context};
use bitcode::read::Read;
use bytes::Bytes;
use tracing::debug;

use crate::channel::channel::ChannelContainer;
use crate::channel::receivers::ChannelReceive;
use crate::channel::senders::ChannelSend;
use crate::packet::message::{FragmentData, MessageContainer, SingleData};
use crate::packet::packet::{Packet, PacketData};
use crate::packet::packet_manager::{PacketManager, PACKET_BUFFER_CAPACITY};
use crate::packet::wrapping_id::{MessageId, PacketId};
use crate::protocol::registry::NetId;
use crate::protocol::Protocol;
use crate::serialize::reader::ReadBuffer;
use crate::transport::{PacketReader, PacketReceiver, PacketSender, Transport};
use crate::{
    BitSerializable, Channel, ChannelKind, ChannelRegistry, ReadWordBuffer, WriteBuffer,
    WriteWordBuffer,
};

/// Wrapper to: send/receive messages via channels to a remote address
/// By splitting the data into packets and sending them through a given transport
// TODO: put the M here or in the functions?
pub struct MessageManager<M: BitSerializable> {
    /// Handles sending/receiving packets (including acks)
    packet_manager: PacketManager,
    // TODO: add ordering of channels per priority
    channels: HashMap<ChannelKind, ChannelContainer>,
    // TODO: can use Vec<ChannelKind, Vec<MessageId>> to be more efficient?
    /// Map to keep track of which messages have been sent in which packets, so that
    /// reliable senders can stop trying to send a message that has already been received
    packet_to_message_id_map: HashMap<PacketId, HashMap<ChannelKind, Vec<MessageId>>>,
    writer: WriteWordBuffer,

    // MessageManager works because we only are only sending a single enum type
    _marker: PhantomData<M>,
}

impl<M: BitSerializable> MessageManager<M> {
    pub fn new(channel_registry: &ChannelRegistry) -> Self {
        Self {
            packet_manager: PacketManager::new(channel_registry.kind_map()),
            channels: channel_registry.channels(),
            packet_to_message_id_map: HashMap::new(),
            writer: WriteWordBuffer::with_capacity(PACKET_BUFFER_CAPACITY),
            _marker: Default::default(),
        }
    }

    /// Buffer a message to be sent on this connection
    pub fn buffer_send(&mut self, message: M, channel_kind: ChannelKind) -> anyhow::Result<()> {
        // debug!("Buffering message to channel");
        let mut channel = self
            .channels
            .get_mut(&channel_kind)
            .context("Channel not found")?;
        self.writer.start_write();
        message.encode(&mut self.writer)?;
        let message_bytes: Vec<u8> = self.writer.finish_write().into();
        channel.sender.buffer_send(message_bytes.into());
        Ok(())
    }

    /// Prepare buckets from the internal send buffers, and return the bytes to send
    // pub fn send_packets(&mut self, io: &mut impl PacketSender) -> anyhow::Result<()> {
    pub fn send_packets(&mut self) -> anyhow::Result<Vec<impl WriteBuffer>> {
        // TODO: all this feels overly complicated.
        //  ideally the packet manager would get a hashmap<channel, list of messages>
        //  and return a list of packets (fragmented or not) that we need.

        //  The problem right now is rust ownership, and the fact that packet_manager
        //  is included in message manager?
        //  Maybe they should be separated and packet manager is passed as an argument

        // Step 1. Get the list of packets to send from all channels
        // for each channel, prepare packets using the buffered messages that are ready to be sent

        // TODO: iterate through the channels in order of channel priority? (with accumulation)
        let mut data_to_send: BTreeMap<NetId, (VecDeque<SingleData>, VecDeque<FragmentData>)> =
            BTreeMap::new();
        for (channel_kind, channel) in self.channels.iter_mut() {
            let channel_id = self
                .packet_manager
                .channel_kind_map
                .net_id(channel_kind)
                .context("cannot find channel id")?;
            channel.sender.collect_messages_to_send();
            if channel.sender.has_messages_to_send() {
                data_to_send.insert(*channel_id, channel.sender.send_packet());
                // // start a new channel in the current packet.
                // // If there's not enough space, start writing in a new packet
                // if !self.packet_manager.can_add_channel(*channel_kind)? {
                //     self.packet_manager.build_new_single_packet();
                //     // can add channel starts writing a new packet if needed
                //     let added_new_channel = self.packet_manager.can_add_channel(*channel_kind)?;
                //     debug_assert!(added_new_channel);
                // }
                // channel.sender.send_packet(&mut self.packet_manager);
            }
        }

        // // TODO: iterate through the channels in order of channel priority? (with accumulation)
        // for (channel_kind, channel) in self.channels.iter_mut() {
        //     channel.sender.collect_messages_to_send();
        //     if channel.sender.has_messages_to_send() {
        //         // start a new channel in the current packet.
        //         // If there's not enough space, start writing in a new packet
        //         if !self.packet_manager.can_add_channel(*channel_kind)? {
        //             self.packet_manager.build_new_single_packet();
        //             // can add channel starts writing a new packet if needed
        //             let added_new_channel = self.packet_manager.can_add_channel(*channel_kind)?;
        //             debug_assert!(added_new_channel);
        //         }
        //         channel.sender.send_packet(&mut self.packet_manager);
        //     }
        // }

        let packets = self.packet_manager.build_packets(data_to_send);

        // TODO: might need to split into single packets?
        let mut bytes = Vec::new();
        for packet in packets {
            // debug!(?packet, "Sending packet");
            // Step 2. Send the packets over the network
            let payload = self.packet_manager.encode_packet(&packet)?;
            bytes.push(payload);
            // io.send(payload, &self.remote_addr)?;

            // TODO: should we update this to include fragment info as well?
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
        let packet: Packet = self.packet_manager.decode_packet(reader)?;
        dbg!(&packet);

        // TODO: if it's fragmented, put it in a buffer? while we wait for all the parts to be ready?
        //  maybe the channel can handle the fragmentation?

        // TODO: an option is to have an async task that is on the receiving side of the
        //  cross-beam channel which tell which packets have been received

        // Step 2. Update the packet acks (which packets have we received, and which of our packets
        // have been acked)
        let acked_packets = self
            .packet_manager
            .header_manager
            .process_recv_packet_header(packet.header());

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
        for (channel_net_id, messages) in packet.data.contents() {
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
                dbg!(&message);
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
            while let Some(single_data) = channel.receiver.read_message() {
                let mut reader = ReadWordBuffer::start_read(single_data.bytes.as_ref());
                let message = M::decode(&mut reader).expect("Could not decode message");
                // TODO: why do we need finish read?
                // reader.finish_read()?;
                messages.push(message);
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
        MessageManager, Protocol, ReadBuffer, ReadWordBuffer, WriteBuffer,
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
    fn test_message_manager() -> Result<(), anyhow::Error> {
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
        let mut client_message_manager =
            MessageManager::<MyMessageProtocol>::new(&channel_registry);
        let mut server_message_manager =
            MessageManager::<MyMessageProtocol>::new(&channel_registry);

        // client: buffer send messages, and then send
        let mut message = MyMessageProtocol::Message1(Message1(1));
        let channel_kind_1 = ChannelKind::of::<Channel1>();
        let channel_kind_2 = ChannelKind::of::<Channel2>();
        client_message_manager.buffer_send(message.clone(), channel_kind_1)?;
        client_message_manager.buffer_send(message.clone(), channel_kind_2)?;
        let mut packet_bytes = client_message_manager.send_packets()?;
        assert_eq!(
            client_message_manager.packet_to_message_id_map,
            HashMap::from([(
                PacketId(0),
                HashMap::from([(channel_kind_1.clone(), vec![MessageId(0)])])
            )])
        );

        // server: receive bytes from the sent messages, then process them into messages
        for mut packet_byte in packet_bytes.iter_mut() {
            server_message_manager
                .recv_packet(&mut ReadWordBuffer::start_read(&packet_byte.finish_write()))?;
        }
        let mut data = server_message_manager.read_messages();
        assert_eq!(data.get(&channel_kind_1).unwrap(), &vec![message.clone()]);
        assert_eq!(data.get(&channel_kind_2).unwrap(), &vec![message.clone()]);

        // Confirm what happens if we try to receive but there is nothing on the io
        data = server_message_manager.read_messages();
        assert!(data.is_empty());

        // Check the state of the packet headers
        assert_eq!(
            client_message_manager
                .packet_manager
                .header_manager
                .next_packet_id(),
            PacketId(1)
        );
        assert!(client_message_manager
            .packet_manager
            .header_manager
            .sent_packets_not_acked()
            .contains(&PacketId(0)));

        // Server sends back a message
        server_message_manager.buffer_send(message.clone(), channel_kind_1)?;
        let mut packet_bytes = server_message_manager.send_packets()?;

        // On client side: keep looping to receive bytes on the network, then process them into messages
        for mut packet_byte in packet_bytes.iter_mut() {
            client_message_manager
                .recv_packet(&mut ReadWordBuffer::start_read(&packet_byte.finish_write()))?;
        }

        // Check that reliability works correctly
        assert_eq!(client_message_manager.packet_to_message_id_map.len(), 0);
        // TODO: check that client_channel_1's sender's unacked messages is empty
        // let client_channel_1 = client_connection.channels.get(&channel_kind_1).unwrap();
        // assert_eq!(client_channel_1.sender.)
        Ok(())
    }
}
