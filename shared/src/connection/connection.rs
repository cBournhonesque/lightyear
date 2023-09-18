use crate::channel::channel::{Channel, ChannelKind};
use crate::packet::header::PacketHeaderManager;
use crate::packet::manager::PacketManager;
use crate::packet::message::Message;
use crate::packet::packet::SinglePacket;
use crate::transport::Transport;
use anyhow::anyhow;
use std::collections::HashMap;
use std::net::SocketAddr;

/// Wrapper to send/receive messages via channels
pub struct Connection {
    /// Handles sending/receiving packets (including acks)
    packet_manager: PacketManager,
    channels: HashMap<ChannelKind, Channel>,
    transport: Box<dyn Transport>,
}

impl Connection {
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
    pub fn send_packets(&mut self, remote_addr: &SocketAddr) -> anyhow::Result<()> {
        // Step 1. Get the list of packets to send from all channels
        // TODO: currently each channel creates separate packets
        //  but actually we could put messages from multiple channels in the same packet
        //  and use a map from packet_id to message_id/channel to decide if we need to re-send
        let mut packets = vec![];
        for channel in self.channels.values_mut() {
            // get the packets from the channel
            let channel_packets = channel.sender.send_packet(&mut self.packet_manager);

            // split them into single packets
            packets.extend(channel_packets.iter().flat_map(|packet| packet.split()));
        }

        // Step 2. Send the packets over the network
        for packet in packets {
            let payload = packet.serialize()?;
            self.transport.send(payload.as_ref(), remote_addr)?;
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

    #[test]
    fn test_connection() {}
}
