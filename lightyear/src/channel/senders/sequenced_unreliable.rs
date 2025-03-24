use alloc::collections::VecDeque;
#[cfg(not(feature = "std"))]
use alloc::{vec, vec::Vec};
use bevy::time::{Timer, TimerMode};
use core::time::Duration;

use bytes::Bytes;
use crossbeam_channel::{Receiver, Sender};

use crate::channel::senders::fragment_sender::FragmentSender;
use crate::channel::senders::ChannelSend;
use crate::packet::message::{MessageAck, MessageData, MessageId, SendMessage, SingleData};
use crate::serialize::SerializationError;
use crate::shared::ping::manager::PingManager;
use crate::shared::tick_manager::TickManager;
use crate::shared::time_manager::TimeManager;

/// A sender that simply sends the messages without checking if they were received
/// Same as UnorderedUnreliableSender, but includes ordering information (MessageId)
#[derive(Debug)]
pub struct SequencedUnreliableSender {
    /// list of single messages that we want to fit into packets and send
    single_messages_to_send: VecDeque<SendMessage>,
    /// list of fragmented messages that we want to fit into packets and send
    fragmented_messages_to_send: VecDeque<SendMessage>,

    /// Message id to use for the next message to be sent
    next_send_message_id: MessageId,
    /// Used to split a message into fragments if the message is too big
    fragment_sender: FragmentSender,
    /// List of senders that want to be notified when a message is lost
    nack_senders: Vec<Sender<MessageId>>,
    /// Internal timer to determine if the channel is ready to send messages
    timer: Option<Timer>,
}

impl SequencedUnreliableSender {
    pub(crate) fn new(send_frequency: Duration) -> Self {
        let timer = if send_frequency == Duration::default() {
            None
        } else {
            Some(Timer::new(send_frequency, TimerMode::Repeating))
        };
        Self {
            single_messages_to_send: VecDeque::new(),
            fragmented_messages_to_send: VecDeque::new(),
            next_send_message_id: MessageId(0),
            fragment_sender: FragmentSender::new(),
            nack_senders: vec![],
            timer,
        }
    }
}

impl ChannelSend for SequencedUnreliableSender {
    fn update(&mut self, time_manager: &TimeManager, _: &PingManager, _: &TickManager) {
        if let Some(timer) = &mut self.timer {
            timer.tick(time_manager.delta());
        }
    }

    /// Add a new message to the buffer of messages to be sent.
    /// This is a client-facing function, to be called when you want to send a message
    fn buffer_send(
        &mut self,
        message: Bytes,
        priority: f32,
    ) -> Result<Option<MessageId>, SerializationError> {
        let message_id = self.next_send_message_id;
        if message.len() > self.fragment_sender.fragment_size {
            for fragment in self
                .fragment_sender
                .build_fragments(message_id, None, message)?
            {
                self.fragmented_messages_to_send.push_back(SendMessage {
                    data: MessageData::Fragment(fragment),
                    priority,
                });
            }
        } else {
            let single_data = SingleData::new(Some(message_id), message);
            self.single_messages_to_send.push_back(SendMessage {
                data: MessageData::Single(single_data),
                priority,
            });
        }
        self.next_send_message_id += 1;
        Ok(Some(message_id))
    }

    /// Take messages from the buffer of messages to be sent, and build a list of packets
    /// to be sent
    fn send_packet(&mut self) -> (VecDeque<SendMessage>, VecDeque<SendMessage>) {
        if self.timer.as_ref().is_some_and(|t| !t.finished()) {
            return (VecDeque::new(), VecDeque::new());
        }
        (
            core::mem::take(&mut self.single_messages_to_send),
            core::mem::take(&mut self.fragmented_messages_to_send),
        )
        // let messages_to_send = core::mem::take(&mut self.messages_to_send);
        // let (remaining_messages_to_send, _) =
        //     packet_manager.pack_messages_within_channel(messages_to_send);
        // self.messages_to_send = remaining_messages_to_send;
    }

    fn receive_ack(&mut self, _message_ack: &MessageAck) {}

    fn subscribe_acks(&mut self) -> Receiver<MessageId> {
        unreachable!()
    }

    /// Create a new receiver that will receive a message id when a sent message on this channel
    /// has been lost by the remote peer
    fn subscribe_nacks(&mut self) -> Receiver<MessageId> {
        let (sender, receiver) = crossbeam_channel::unbounded();
        self.nack_senders.push(sender);
        receiver
    }

    /// Send nacks to the subscribers of nacks
    fn send_nacks(&mut self, nack: MessageId) {
        for sender in &self.nack_senders {
            sender.send(nack).unwrap();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prelude::{PingConfig, TickConfig};
    #[test]
    fn test_sequenced_unreliable_sender_internals() {
        let mut sender = SequencedUnreliableSender::new(Duration::from_secs(1));
        assert!(sender.timer.as_ref().is_some_and(|t| !t.finished()));

        sender.buffer_send(Bytes::from("hello"), 1.0).unwrap();

        // we do not send because we didn't reach the timer
        let (single, _) = sender.send_packet();
        assert!(single.is_empty());

        // update with a delta of 1 second
        let mut time_manager = TimeManager::default();
        time_manager.update(Duration::from_secs(1));
        sender.update(
            &time_manager,
            &PingManager::new(PingConfig::default()),
            &TickManager::from_config(TickConfig::new(Duration::from_secs(1))),
        );

        // this time, we send the packet
        let (single, _) = sender.send_packet();
        assert_eq!(single.len(), 1);
    }
}
