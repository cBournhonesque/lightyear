use crate::channel::channel::{Channel, ChannelKind};
use crate::channel::receivers::ChannelReceive;
use crate::channel::senders::ChannelSend;
use crate::packet::header::PacketHeaderManager;
use crate::packet::manager::PacketManager;
use crate::packet::message::Message;
use crate::packet::packet::SinglePacket;
use crate::transport::Transport;
use anyhow::anyhow;
use std::collections::HashMap;
use std::net::SocketAddr;

/// Wrapper to: send/receive messages via channels to a remote address
/// By splitting the data into packets and sending them through a given transport
pub struct Connection {
    /// Handles sending/receiving packets (including acks)
    packet_manager: PacketManager,
    channels: HashMap<ChannelKind, Channel>,
    transport: Box<dyn Transport>,
    remote_addr: SocketAddr,
}

impl Connection {
    pub fn new(remote_addr: SocketAddr, transport: Box<dyn Transport>) -> Self {
        Self {
            packet_manager: PacketManager::new(),
            channels: Default::default(),
            transport,
            remote_addr,
        }
    }

    /// Buffer a message to be sent on this connection
    pub fn buffer_send(
        &mut self,
        message: Message,
        channel_kind: ChannelKind,
    ) -> anyhow::Result<()> {
        let channel = self
            .channels
            .get_mut(&channel_kind)
            .ok_or_else(|| anyhow!("channel not found"))?;
        Ok(channel.sender.buffer_send(message))
    }

    /// Prepare buckets from the internal send buffers, and send them over the network
    pub fn send_packets(&mut self) -> anyhow::Result<()> {
        // Step 1. Get the list of packets to send from all channels
        // TODO: currently each channel creates separate packets
        //  but actually we could put messages from multiple channels in the same packet
        //  and use a map from packet_id to message_id/channel to decide if we need to re-send
        let mut packets = vec![];
        for channel in self.channels.values_mut() {
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
            let payload = packet.serialize()?;
            self.transport.send(payload.as_ref(), &self.remote_addr)?;
        }

        Ok(())
    }

    /// Process packet received over the network as raw bytes
    /// Update the acks, and put the messages from the packets in internal buffers
    pub fn recv_packet(&mut self, packet: &[u8]) -> anyhow::Result<()> {
        // Step 1. Parse the packet
        let packet = SinglePacket::deserialize(packet)?;

        // Step 2. Update the packet acks (which packets have we received, and which of our packets
        // have been acked)
        self.packet_manager
            .header_manager
            .process_recv_packet_header(&packet.header);

        // Step 3. Put the messages from the packet in the internal buffers
        let channel_kind = packet.header.channel_header.kind;
        let channel = self
            .channels
            .get_mut(&channel_kind)
            .ok_or_else(|| anyhow!("channel not found"))?;

        for message in packet.data {
            channel.receiver.buffer_recv(message)?;
        }

        // TODO: should we have a mapping from packet_id to message_id?

        Ok(())
    }

    /// Read all the messages in the internal buffers that are ready to be processed
    // TODO: this is where naia converts the messages to events and pushes them to an event queue
    //  lets be conservative and just return the messages right now. We could switch to an iterator
    pub fn read_messages(&mut self) -> Vec<Message> {
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
    use crate::transport::udp::Socket;
    use crate::transport::Transport;
    use std::net::SocketAddr;
    use std::str::FromStr;

    #[test]
    /// We want to test that we can send/receive messages over a connection
    fn test_connection() -> Result<(), anyhow::Error> {
        // Create connections
        let socket_addr = SocketAddr::from_str("127.0.0.1:0")?;
        let mut server_socket = Socket::new(&socket_addr)?;
        let client_socket = Socket::new(&socket_addr)?;
        let server_addr = server_socket.local_addr()?;
        let client_addr = client_socket.local_addr()?;

        let client_transport = Box::new(Socket::new(&client_addr));
        let mut client_connection = Connection::new(server_addr, client_transport);

        let server_transport = Box::new(Socket::new(&server_addr));
        let mut server_connection = Connection::new(client_addr, server_transport);

        // Add channels (ideally we create them only once via a shared protocol)

        // On client side: buffer send messages, and then send
        // On server side: keep looping to receive bytes, then process them into messages

        // Check that the received messages are the same as the sent ones
        // Maybe inspect how the messages were put into packets? (or this could be a test for packet writer)
    }
}
