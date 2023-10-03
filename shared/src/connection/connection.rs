use std::collections::HashMap;
use std::net::SocketAddr;

use anyhow::{anyhow, bail, Context};
use bitcode::read::Read;

use crate::channel::channel::ChannelContainer;
use crate::channel::receivers::ChannelReceive;
use crate::channel::senders::ChannelSend;
use crate::connection::io::Io;
use crate::packet::manager::PacketManager;
use crate::packet::message::MessageContainer;
use crate::packet::packet::Packet;
use crate::protocol::Protocol;
use crate::registry::channel::{ChannelKind, ChannelRegistry};
use crate::serialize::reader::ReadBuffer;
use crate::transport::Transport;
use crate::Channel;

/// Wrapper to: send/receive messages via channels to a remote address
/// By splitting the data into packets and sending them through a given transport
pub struct Connection<P: Protocol> {
    /// Handles sending/receiving packets (including acks)
    packet_manager: PacketManager<P::Message>,
    // TODO: add ordering of channels per priority
    channels: HashMap<ChannelKind, ChannelContainer<P::Message>>,
    remote_addr: SocketAddr,
}

impl<P: Protocol> Connection<P> {
    pub fn new(remote_addr: SocketAddr, channel_registry: &'static ChannelRegistry) -> Self {
        Self {
            packet_manager: PacketManager::new(channel_registry),
            channels: channel_registry.channels(),
            remote_addr,
        }
    }

    /// Buffer a message to be sent on this connection
    pub fn buffer_send(
        &mut self,
        message: MessageContainer<P::Message>,
        channel_kind: ChannelKind,
    ) -> anyhow::Result<()> {
        let mut channel = self
            .channels
            .get_mut(&channel_kind)
            .context("Channel not found")?;
        Ok(channel.sender.buffer_send(message))
    }

    /// Prepare buckets from the internal send buffers, and send them over the network
    pub fn send_packets(&mut self, io: &mut Io) -> anyhow::Result<()> {
        // Step 1. Get the list of packets to send from all channels
        // TODO: currently each channel creates separate packets
        //  but actually we could put messages from multiple channels in the same packet
        //  and use a map from packet_id to message_id/channel to decide if we need to re-send
        //  all the messages that were sent through a non-reliable channel don't need to be re-sent
        // for each channel, prepare packets using the buffered messages that are ready to be sent
        for (channel_kind, channel) in self.channels.iter_mut() {
            channel.sender.collect_messages_to_send();
            if channel.sender.has_messages_to_send() {
                // start a new channel in the current packet.
                // If there's not enough space, start writing in a new packet
                if !self.packet_manager.can_add_channel(channel_kind.clone())? {
                    self.packet_manager.build_new_packet();
                    // can add channel starts writing a new packet if needed
                    let added_new_channel =
                        self.packet_manager.can_add_channel(channel_kind.clone())?;
                    debug_assert!(added_new_channel);
                }
                channel.sender.send_packet(&mut self.packet_manager);
            }
        }

        let packets = self.packet_manager.flush_packets();

        // TODO: might need to split into single packets?
        // Step 2. Send the packets over the network
        for packet in packets {
            let payload = self.packet_manager.encode_packet(&packet)?;
            io.send_packet(payload, &self.remote_addr)?;
        }

        Ok(())
    }

    /// Listen for packets on the transport and buffer them
    ///
    /// Return when there are no more packets to receive on the transport
    pub fn listen(&mut self, io: &mut Io) -> anyhow::Result<()> {
        loop {
            match io.create_reader_from_packet()? {
                Some((mut packet_reader, address)) => {
                    if address != self.remote_addr {
                        bail!("received packet from unknown address");
                    }
                    self.recv_packet(&mut packet_reader)?;
                    continue;
                }
                None => break,
            }
        }
        Ok(())
    }

    /// Process packet received over the network as raw bytes
    /// Update the acks, and put the messages from the packets in internal buffers
    fn recv_packet(&mut self, reader: &mut impl ReadBuffer) -> anyhow::Result<()> {
        // Step 1. Parse the packet
        let packet: Packet<P::Message> = self.packet_manager.decode_packet(reader)?;
        let packet = match packet {
            Packet::Single(single_packet) => single_packet,
            Packet::Fragmented(_) => unimplemented!(),
        };

        // Step 2. Update the packet acks (which packets have we received, and which of our packets
        // have been acked)
        self.packet_manager
            .header_manager
            .process_recv_packet_header(&packet.header);

        // Step 3. Put the messages from the packet in the internal buffers for each channel
        for (channel_net_id, messages) in packet.data {
            let channel_kind = self
                .packet_manager
                .channel_registry
                .get_kind_from_net_id(channel_net_id)
                .context(format!(
                    "Could not recognize net_id {} as a channel",
                    channel_net_id
                ))?;
            let channel = self
                .channels
                .get_mut(&channel_kind)
                .ok_or_else(|| anyhow!("Channel not found"))?;
            for message in messages {
                channel.receiver.buffer_recv(message)?;
            }
        }

        // TODO: should we have a mapping from packet_id to message_id?
        Ok(())
    }

    /// Read all the messages in the internal buffers that are ready to be processed
    // TODO: this is where naia converts the messages to events and pushes them to an event queue
    //  lets be conservative and just return the messages right now. We could switch to an iterator
    pub fn read_messages(&mut self) -> HashMap<ChannelKind, Vec<MessageContainer<P::Message>>> {
        let mut map = HashMap::new();
        for (channel_kind, channel) in self.channels.iter_mut() {
            let mut messages = vec![];
            while let Some(message) = channel.receiver.read_message() {
                messages.push(message);
            }
            if !messages.is_empty() {
                map.insert(channel_kind.clone(), messages);
            }
        }
        map
    }
}

// TODO: have a way to update the channels about the messages that have been acked

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;
    use std::str::FromStr;
    use std::time::Duration;

    use lazy_static::lazy_static;
    use serde::{Deserialize, Serialize};

    use lightyear_derive::ChannelInternal;

    use crate::channel::channel::ReliableSettings;
    use crate::connection::connection::Connection;
    use crate::connection::io::Io;
    use crate::packet::wrapping_id::{MessageId, PacketId};
    use crate::transport::udp::Socket;
    use crate::transport::Transport;
    use crate::{
        ChannelDirection, ChannelKind, ChannelMode, ChannelRegistry, ChannelSettings,
        MessageContainer, Protocol,
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

    pub enum MyProtocol {
        MyMessageProtocol(MyMessageProtocol),
    }

    impl Protocol for MyProtocol {
        type Message = MyMessageProtocol;
    }

    // Channels
    #[derive(ChannelInternal)]
    struct Channel1;

    #[derive(ChannelInternal)]
    struct Channel2;

    lazy_static! {
        static ref CHANNEL_REGISTRY: ChannelRegistry = {
            let mut c = ChannelRegistry::new();
            c.add::<Channel1>(ChannelSettings {
                mode: ChannelMode::OrderedReliable(ReliableSettings::default()),
                direction: ChannelDirection::Bidirectional,
            })
            .unwrap();
            c.add::<Channel2>(ChannelSettings {
                mode: ChannelMode::UnorderedUnreliable,
                direction: ChannelDirection::Bidirectional,
            })
            .unwrap();
            c
        };
    }

    #[test]
    /// We want to test that we can send/receive messages over a connection
    fn test_connection() -> Result<(), anyhow::Error> {
        // Create connections
        let socket_addr = SocketAddr::from_str("127.0.0.1:0")?;
        let server_socket = Socket::new(&socket_addr)?;
        let client_socket = Socket::new(&socket_addr)?;
        let server_addr = server_socket.local_addr()?;
        let client_addr = client_socket.local_addr()?;

        dbg!(server_addr);
        dbg!(client_addr);

        let mut client_io = Io::new(Box::new(client_socket));
        let mut client_connection = Connection::<MyProtocol>::new(server_addr, &CHANNEL_REGISTRY);

        let mut server_io = Io::new(Box::new(server_socket));
        let mut server_connection = Connection::<MyProtocol>::new(client_addr, &CHANNEL_REGISTRY);

        // On client side: buffer send messages, and then send
        let with_id = |message: &MessageContainer<_>, id| {
            let mut m = message.clone();
            m.set_id(MessageId(id));
            m
        };

        let mut message = MessageContainer::new(MyMessageProtocol::Message1(Message1(1)));
        let channel_kind_1 = ChannelKind::of::<Channel1>();
        let channel_kind_2 = ChannelKind::of::<Channel2>();
        client_connection.buffer_send(message.clone(), ChannelKind::of::<Channel1>())?;
        client_connection.buffer_send(message.clone(), ChannelKind::of::<Channel2>())?;
        client_connection.send_packets(&mut client_io)?;

        // Sleep to make sure the server receives the message
        std::thread::sleep(Duration::from_millis(100));

        // On server side: keep looping to receive bytes on the network, then process them into messages
        server_connection.listen(&mut server_io);
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
            .contains_key(&PacketId(0)));
        Ok(())
    }
}
