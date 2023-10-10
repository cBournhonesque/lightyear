use lightyear_shared::netcode::Client as ClientIO;
use lightyear_shared::transport::{PacketReader, PacketReceiver};
use lightyear_shared::{
    ChannelKind, Connection, Io, MessageContainer, Protocol, ReadBuffer, ReadWordBuffer,
};
use renetcode::NetcodeError;
use std::collections::HashMap;
use std::io::Read;
use std::time::Duration;

pub(crate) struct ClientId(pub u32);

pub struct Client<P: Protocol> {
    io: ClientIO,
    message_manager: Connection<P>,
}

impl<P: Protocol> Client<P> {
    pub fn new(io: ClientIO, message_manager: Connection<P>) -> Self {
        Self {
            io,
            message_manager,
        }
    }

    // ///
    // pub fn update(&mut self, duration: Duration) -> anyhow::Result<()> {
    //     // Check for disconnects (on netcode or on transport)
    //
    //     // Receive packets from transport (and store them in buffers)
    //     self.recv_packets()?;
    //
    //     // TODO: maybe have send packets into a separate function, like renet
    //     // Send packets to transport
    //     self.send_packets()?;
    //
    //     // Update the internal state of the netcode transport (and possibly send some protocol packet)
    //     self.io.update(duration)?;
    //     Ok(())
    // }
    //
    /// Send a message to the server
    pub fn buffer_send(
        &mut self,
        message: MessageContainer<P::Message>,
        channel_kind: ChannelKind,
    ) -> anyhow::Result<()> {
        self.message_manager.buffer_send(message, channel_kind)
    }

    /// Receive messages from the server
    /// TODO: maybe use events?
    pub fn read_messages(&mut self) -> HashMap<ChannelKind, Vec<MessageContainer<P::Message>>> {
        self.message_manager.read_messages()
    }

    /// Send packets that are ready from the message manager through the transport layer
    pub fn send_packets(&mut self) -> anyhow::Result<()> {
        let io = &mut self.io;
        self.message_manager.send_packets(io)
    }

    /// Receive packets from the transport layer and buffer them with the message manager
    pub fn recv_packets(&mut self) -> anyhow::Result<()> {
        let io = &mut self.io;
        loop {
            match io.recv_packet()? {
                Some(buf) => {
                    let mut reader = ReadWordBuffer::start_read(buf);
                    self.message_manager.recv_packet(&mut reader)?;
                }
                None => break,
            }
        }
        Ok(())
    }
}
