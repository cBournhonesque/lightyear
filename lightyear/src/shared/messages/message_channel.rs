use lightyear_serde::{BitReader, BitWrite, BitWriter, Error};
use naia_socket_shared::Instant;

use crate::shared::types::MessageIndex;

/// General trait for a channel that sends messages
pub trait ChannelSender<P>: Send + Sync {
    /// Queues a Message to be transmitted to the remote host into an internal buffer
    fn buffer_message(&mut self, message: P);
    /// For reliable channels, will collect any Messages that need to be resent
    fn collect_messages(&mut self, now: &Instant, rtt_millis: &f32);
    /// Returns true if there are queued Messages ready to be written
    fn has_messages(&self) -> bool;
    /// Gets Messages from the internal buffer and writes it to the channel_writer
    fn write_messages(
        &mut self,
        channel_writer: &dyn ChannelWriter<P>,
        bit_writer: &mut BitWriter,
        has_written: &mut bool,
    ) -> Option<Vec<MessageIndex>>;
    /// Called when it receives acknowledgement that a Message has been received
    fn notify_message_delivered(&mut self, message_id: &MessageIndex);
}

/// General trait for a channel that reads messages
pub trait ChannelReceiver<P>: Send + Sync {
    /// Read messages from raw bits, parse them and store then into an internal buffer
    fn read_messages(
        &mut self,
        channel_reader: &dyn ChannelReader<P>,
        reader: &mut BitReader,
    ) -> Result<(), Error>;
    /// Read messages from an internal buffer and return their content
    fn receive_messages(&mut self) -> Vec<P>;
}

pub trait ChannelWriter<T> {
    /// Writes a Message into the outgoing packet
    fn write(&self, writer: &mut dyn BitWrite, data: &T);
}

pub trait ChannelReader<T> {
    /// Reads a Message from an incoming packet
    fn read(&self, reader: &mut BitReader) -> Result<T, Error>;
}
