use crate::channel::channel::ChannelContainer;
use crate::channel::receivers::ChannelReceive;
use crate::channel::senders::ChannelSend;
use crate::packet::manager::PacketManager;
use crate::packet::message::MessageContainer;
use crate::packet::packet::Packet;
use crate::registry::channel::{ChannelKind, ChannelRegistry};
use crate::registry::message::MessageRegistry;
use crate::transport::Transport;
use crate::Channel;
use anyhow::{anyhow, bail, Context};
use bitcode::read::Read;
use std::collections::HashMap;
use std::net::SocketAddr;

/// Wrapper to: send/receive messages via channels to a remote address
/// By splitting the data into packets and sending them through a given transport
pub struct Connection {
    /// Handles sending/receiving packets (including acks)
    packet_manager: PacketManager,
    channel_registry: &'static ChannelRegistry,
    channels: HashMap<ChannelKind, ChannelContainer>,
    // transport: Box<RefCell<dyn Transport>>,
    remote_addr: SocketAddr,
}

impl Connection {
    // pub fn new(remote_addr: SocketAddr, transport: Box<dyn Transport>) -> Self {
    pub fn new(
        remote_addr: SocketAddr,
        channel_registry: &ChannelRegistry,
        message_registry: &MessageRegistry,
    ) -> Self {
        Self {
            packet_manager: PacketManager::new(channel_registry, message_registry),
            channel_registry,
            channels: channel_registry.channels(),
            // transport: Box::new(RefCell::new(transport.as_ref())),
            remote_addr,
        }
    }

    /// Buffer a message to be sent on this connection
    pub fn buffer_send(
        &mut self,
        message: MessageContainer,
        channel_kind: ChannelKind,
    ) -> anyhow::Result<()> {
        let mut channel = self
            .channels
            .get_mut(&channel_kind)
            .context("Channel not found")?;
        Ok(channel.sender.buffer_send(message))
    }

    /// Prepare buckets from the internal send buffers, and send them over the network
    pub fn send_packets(&mut self, transport: Box<dyn Transport>) -> anyhow::Result<()> {
        // Step 1. Get the list of packets to send from all channels
        // TODO: currently each channel creates separate packets
        //  but actually we could put messages from multiple channels in the same packet
        //  and use a map from packet_id to message_id/channel to decide if we need to re-send
        //  all the messages that were sent through a non-reliable channel don't need to be re-sent
        let mut packets = vec![];
        for channel in self.channels.values_mut() {
            // TODO: need to write the channel id in the packet! maybe need to add the ChannelKind in send_packet?
            // get the packets from the channel
            let channel_packets = channel.sender.send_packet(&mut self.packet_manager);

            // split them into single packets
            packets.extend(
                channel_packets
                    .into_iter()
                    .flat_map(|packet| packet.split()),
            );
        }

        // Step 2. Send the packets over the network
        for packet in packets {
            let payload = self.packet_manager.encode_packet(&Packet::Single(packet))?;
            transport.send(payload, &self.remote_addr)?;
        }

        Ok(())
    }

    /// Listen for packets on the transport and buffer them
    ///
    /// Return when there are no more packets to receive on the transport
    pub fn listen(&mut self, mut transport: Box<dyn Transport>) -> anyhow::Result<()> {
        loop {
            match transport.recv()? {
                Some((recv_len, address)) => {
                    if address != self.remote_addr {
                        bail!("received packet from unknown address");
                    }
                    self.recv_packet(recv_len)?;
                    continue;
                }
                None => break,
            }
        }
        // loop {
        //     match self.transport.recv()? {
        //         Some((recv_len, address)) => {
        //             if address != self.remote_addr {
        //                 bail!("received packet from unknown address");
        //             }
        //             self.recv_packet(recv_len)?;
        //             continue;
        //         }
        //         None => break,
        //     }
        // }
        Ok(())
    }

    /// Process packet received over the network as raw bytes
    /// Update the acks, and put the messages from the packets in internal buffers
    pub fn recv_packet(&mut self, reader: &mut impl Read) -> anyhow::Result<()> {
        // Step 1. Parse the packet
        let packet = self.packet_manager.decode_packet(reader)?;
        let packet = match packet {
            Packet::Single(single_packet) => single_packet,
            Packet::Fragmented(_) => unimplemented!(),
        };

        // Step 2. Update the packet acks (which packets have we received, and which of our packets
        // have been acked)
        self.packet_manager
            .header_manager
            .process_recv_packet_header(&packet.header);

        // Step 3. Put the messages from the packet in the internal buffers
        // TODO
        // let channel_kind = packet.header.channel_header.kind;
        // let channel_kind = ChannelKind(0);
        // let channel = self
        //     .channels
        //     .get_mut(&channel_kind)
        //     .ok_or_else(|| anyhow!("channel not found"))?;
        //
        // for message in packet.data {
        //     channel.receiver.buffer_recv(message)?;
        // }

        // TODO: should we have a mapping from packet_id to message_id?

        Ok(())
    }

    /// Read all the messages in the internal buffers that are ready to be processed
    // TODO: this is where naia converts the messages to events and pushes them to an event queue
    //  lets be conservative and just return the messages right now. We could switch to an iterator
    pub fn read_messages(&mut self) -> Vec<MessageContainer> {
        let mut messages = vec![];

        // TODO: output data about which channel the message came from?
        for channel in self.channels.values_mut() {
            while let Some(message) = channel.receiver.read_message() {
                messages.push(message);
            }
        }

        messages
    }
}

#[cfg(test)]
mod tests {
    use super::Connection;
    use crate::channel::channel::ChannelMode::OrderedReliable;
    use crate::channel::channel::{
        ChannelContainer, ChannelDirection, ChannelKind, ChannelSettings, ReliableSettings,
    };
    use crate::packet::message::MessageContainer;
    use crate::packet::wrapping_id::MessageId;
    use crate::transport::udp::Socket;
    use crate::transport::Transport;
    use bytes::Bytes;
    use std::collections::HashMap;
    use std::net::SocketAddr;
    use std::str::FromStr;
    use std::time::Duration;

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

        // Create channels (ideally we create them only once via a shared protocol)
        let channel_kind = ChannelKind::new(0);
        let channel = ChannelContainer::new(ChannelSettings {
            mode: OrderedReliable(ReliableSettings::default()),
            direction: ChannelDirection::Bidirectional,
        });
        let client_channels = HashMap::from([(channel_kind, channel)]);

        let client_transport = Box::new(client_socket);
        let mut client_connection = Connection::new(server_addr, client_channels);

        let channel_kind = ChannelKind::new(0);
        let channel = ChannelContainer::new(ChannelSettings {
            mode: OrderedReliable(ReliableSettings::default()),
            direction: ChannelDirection::Bidirectional,
        });
        let server_channels = HashMap::from([(channel_kind, channel)]);

        let server_transport = Box::new(server_socket);
        let mut server_connection = Connection::new(client_addr, server_channels);

        // On client side: buffer send messages, and then send
        let mut message = MessageContainer::new(Bytes::from("hello"));
        client_connection.buffer_send(message.clone(), channel_kind)?;
        client_connection.send_packets(client_transport)?;

        // Sleep to make sure the server receives the message
        std::thread::sleep(Duration::from_millis(100));

        // On server side: keep looping to receive bytes on the network, then process them into messages
        server_connection.listen(server_transport);
        let messages = server_connection.read_messages();
        message.set_id(MessageId(0));
        assert_eq!(messages, vec![message]);

        // Check that the received messages are the same as the sent ones
        // Maybe inspect how the messages were put into packets? (or this could be a test for packet writer)
        Ok(())
    }
}
