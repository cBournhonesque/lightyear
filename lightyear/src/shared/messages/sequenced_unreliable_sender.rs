use std::collections::VecDeque;

use lightyear_serde::BitWriter;
use naia_socket_shared::Instant;

use crate::shared::{messages::indexed_message_writer::IndexedMessageWriter, types::MessageIndex};

use super::message_channel::{ChannelSender, ChannelWriter};

pub struct SequencedUnreliableSender<P: Send> {
    /// Buffer of the next messages to send along with their MessageId
    outgoing_messages: VecDeque<(MessageIndex, P)>,
    /// Next message id to use (not yet used in the buffer)
    next_send_message_id: MessageIndex,
}

impl<P: Send> SequencedUnreliableSender<P> {
    pub fn new() -> Self {
        Self {
            outgoing_messages: VecDeque::new(),
            next_send_message_id: 0,
        }
    }
}

impl<P: Send + Sync> ChannelSender<P> for SequencedUnreliableSender<P> {
    fn buffer_message(&mut self, message: P) {
        self.outgoing_messages
            .push_back((self.next_send_message_id, message));
        self.next_send_message_id = self.next_send_message_id.wrapping_add(1);
    }

    fn collect_messages(&mut self, _: &Instant, _: &f32) {
        // not necessary for an unreliable channel
    }

    fn has_messages(&self) -> bool {
        !self.outgoing_messages.is_empty()
    }

    /// Write messages from the buffer into the channel
    /// Include a wrapped message id for sequencing purposes
    fn write_messages(
        &mut self,
        channel_writer: &dyn ChannelWriter<P>,
        bit_writer: &mut BitWriter,
        has_written: &mut bool,
    ) -> Option<Vec<MessageIndex>> {
        IndexedMessageWriter::write_messages(
            &mut self.outgoing_messages,
            channel_writer,
            bit_writer,
            has_written,
        )
    }

    fn notify_message_delivered(&mut self, _: &MessageIndex) {
        // not necessary for an unreliable channel
    }
}
