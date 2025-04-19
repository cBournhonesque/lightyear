use alloc::collections::{BTreeMap, VecDeque};
#[cfg(not(feature = "std"))]
use alloc::{vec, vec::Vec};
use bevy::platform::collections::HashSet;
use bevy::prelude::{Timer, TimerMode};

use bytes::Bytes;
use core::time::Duration;
use crossbeam_channel::{Receiver, Sender};
use tracing::trace;

use crate::channel::builder::ReliableSettings;
use crate::channel::senders::fragment_sender::FragmentSender;
use crate::channel::senders::ChannelSend;
use crate::packet::message::{FragmentData, MessageAck, MessageId, SendMessage, SingleData};
use crate::serialize::SerializationError;
use crate::shared::ping::manager::PingManager;
use crate::shared::tick_manager::TickManager;
use crate::shared::time_manager::{TimeManager, WrappedTime};

#[derive(Debug)]
pub struct FragmentAck {
    data: FragmentData,
    acked: bool,
    last_sent: Option<WrappedTime>,
}

/// A message that has not been acked yet
#[derive(Debug)]
pub enum UnackedMessage {
    Single {
        bytes: Bytes,
        /// If None: this packet has never been sent before
        /// else: the last instant when this packet was sent
        last_sent: Option<WrappedTime>,
    },
    Fragmented(Vec<FragmentAck>),
}

#[derive(Debug)]
pub struct UnackedMessageWithPriority {
    pub unacked_message: UnackedMessage,
    pub base_priority: f32,
    pub accumulated_priority: f32,
}

/// A sender that makes sure to resend messages until it receives an ack
#[derive(Debug)]
pub struct ReliableSender {
    /// Settings for reliability
    reliable_settings: ReliableSettings,
    // TODO: maybe optimize by using a RingBuffer
    /// Ordered map of the messages that haven't been acked yet
    unacked_messages: BTreeMap<MessageId, UnackedMessageWithPriority>,
    /// Message id to use for the next message to be sent
    next_send_message_id: MessageId,

    /// list of single messages that we want to fit into packets and send
    single_messages_to_send: VecDeque<SendMessage>,
    /// list of fragmented messages that we want to fit into packets and send
    fragmented_messages_to_send: VecDeque<SendMessage>,

    /// Set of message ids that we want to send (to prevent sending the same message twice)
    /// (includes the [`FragmentIndex`](crate::packet::message::FragmentIndex) for fragments)
    message_ids_to_send: HashSet<MessageAck>,

    /// Used to split a message into fragments if the message is too big
    fragment_sender: FragmentSender,

    /// List of senders that want to be notified when a message is acked
    ack_senders: Vec<Sender<MessageId>>,
    /// List of senders that want to be notified when a message is lost
    nack_senders: Vec<Sender<MessageId>>,
    current_rtt: Duration,
    current_time: WrappedTime,
    /// Internal timer to determine if the channel is ready to send messages
    timer: Option<Timer>,
    /// Factor that makes sure that the priority accumulates at the same right even the channel
    /// sends messages infrequently
    priority_multiplier: f32,
}

impl ReliableSender {
    pub fn new(reliable_settings: ReliableSettings, send_frequency: Duration) -> Self {
        let timer = if send_frequency == Duration::default() {
            None
        } else {
            Some(Timer::new(send_frequency, TimerMode::Repeating))
        };
        Self {
            reliable_settings,
            unacked_messages: Default::default(),
            next_send_message_id: MessageId(0),
            single_messages_to_send: Default::default(),
            fragmented_messages_to_send: Default::default(),
            message_ids_to_send: Default::default(),
            fragment_sender: FragmentSender::new(),
            ack_senders: vec![],
            nack_senders: vec![],
            current_rtt: Duration::default(),
            current_time: WrappedTime::default(),
            timer,
            priority_multiplier: 1.0,
        }
    }
}

impl ChannelSend for ReliableSender {
    fn update(&mut self, time_manager: &TimeManager, ping_manager: &PingManager, _: &TickManager) {
        self.current_time = time_manager.current_time();
        self.current_rtt = ping_manager.rtt();
        if let Some(timer) = &mut self.timer {
            timer.tick(time_manager.delta());
            self.priority_multiplier =
                timer.duration().as_nanos() as f32 / time_manager.delta().as_nanos() as f32;
            trace!(
                ?timer,
                "Priority multiplier for reliable sender channel: {:?}",
                self.priority_multiplier
            );
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
        let unacked_message = if message.len() > self.fragment_sender.fragment_size {
            let fragments = self
                .fragment_sender
                .build_fragments(message_id, None, message)?;
            UnackedMessage::Fragmented(
                fragments
                    .into_iter()
                    .map(|fragment| FragmentAck {
                        data: fragment,
                        acked: false,
                        last_sent: None,
                    })
                    .collect(),
            )
        } else {
            UnackedMessage::Single {
                bytes: message,
                last_sent: None,
            }
        };
        let unacked_message_with_priority = UnackedMessageWithPriority {
            unacked_message,
            base_priority: priority,
            // store with 0.0 accumulated priority because priority gets accumulated when we collect the messages
            // for sending (even the first time the message is sent)
            accumulated_priority: 0.0,
        };
        self.unacked_messages
            .insert(message_id, unacked_message_with_priority);
        self.next_send_message_id += 1;
        Ok(Some(message_id))
    }

    /// Take messages from the buffer of messages to be sent, and build a list of packets
    /// to be sent
    /// The messages to be sent need to have been collected prior to this point.
    fn send_packet(&mut self) -> (VecDeque<SendMessage>, VecDeque<SendMessage>) {
        if self.timer.as_ref().is_some_and(|t| !t.finished()) {
            return (VecDeque::new(), VecDeque::new());
        }

        // Collect the list of messages that need to be sent
        // Either because they have never been sent, or because they need to be resent

        // resend delay is based on the rtt
        let resend_delay =
            chrono::Duration::from_std(self.reliable_settings.resend_delay(self.current_rtt))
                .unwrap();
        let should_send = |last_sent: &Option<WrappedTime>| -> bool {
            match last_sent {
                // send if the message has never been sent
                None => true,
                // or if we sent it a while back but didn't get an ack
                Some(last_sent) => self.current_time - *last_sent > resend_delay,
            }
        };

        // Iterate through all unacked messages, oldest message ids first
        for (message_id, unacked_message_with_priority) in self.unacked_messages.iter_mut() {
            // accumulate the priority for all messages (including the ones that were just added, since we set the accumulated priority to 0.0)
            unacked_message_with_priority.accumulated_priority +=
                unacked_message_with_priority.base_priority * self.priority_multiplier;
            trace!(
                "Accumulating priority for reliable message {:?} to {:?}. Base priority: {:?}, Multiplier: {:?}",
                message_id,
                unacked_message_with_priority.accumulated_priority,
                unacked_message_with_priority.base_priority,
                self.priority_multiplier
            );

            match &mut unacked_message_with_priority.unacked_message {
                UnackedMessage::Single {
                    bytes,
                    last_sent,
                } => {
                    if should_send(last_sent) {
                        trace!("Should send message {:?}", message_id);
                        let message_info = MessageAck {
                            message_id: *message_id,
                            fragment_id: None,
                        };
                        if !self.message_ids_to_send.contains(&message_info) {
                            let message = SingleData::new(Some(*message_id), bytes.clone());
                            self.single_messages_to_send.push_back(SendMessage {
                                data: message.into(),
                                priority: unacked_message_with_priority.accumulated_priority,
                            });
                            self.message_ids_to_send.insert(message_info);
                            *last_sent = Some(self.current_time);
                        }
                    }
                }
                UnackedMessage::Fragmented(fragment_acks) => {
                    // only send the fragments that haven't been acked and should be resent
                    fragment_acks
                        .iter_mut()
                        .filter(|f| !f.acked && should_send(&f.last_sent))
                        .for_each(|f| {
                            let message_info = MessageAck {
                                message_id: *message_id,
                                fragment_id: Some(f.data.fragment_id),
                            };
                            if !self.message_ids_to_send.contains(&message_info) {
                                let message = f.data.clone();
                                self.fragmented_messages_to_send.push_back(SendMessage {
                                    data: message.into(),
                                    priority: unacked_message_with_priority.accumulated_priority,
                                });
                                self.message_ids_to_send.insert(message_info);
                                f.last_sent = Some(self.current_time);
                            }
                        })
                }
            }
        }

        // TODO: is this message_ids_to_send even useful? in which situation would we send the same message twice?
        // right now, we send everything; so we can reset
        self.message_ids_to_send.clear();
        if !self.single_messages_to_send.is_empty() {
            trace!(
                "Single messages to send: {:?}",
                self.single_messages_to_send
            );
        }

        // TODO: use double-buffer to reuse allocated memory?
        (
            core::mem::take(&mut self.single_messages_to_send),
            core::mem::take(&mut self.fragmented_messages_to_send),
        )

        // TODO: handle if we couldn't send all messages?
        // TODO: update message_ids_to_send?
        // TODO: get back the list of messages we could not send?

        // // build the packets from those messages
        // let single_messages_to_send = core::mem::take(&mut self.single_messages_to_send);
        // let (remaining_messages_to_send, sent_message_ids) =
        //     packet_manager.pack_messages_within_channel(messages_to_send);
        // self.messages_to_send = remaining_messages_to_send;
        //
        // for message_id in sent_message_ids {
        //     self.message_ids_to_send.remove(&message_id);
        // }
    }

    fn receive_ack(&mut self, message_ack: &MessageAck) {
        if let Some(unacked_message) = self.unacked_messages.get_mut(&message_ack.message_id) {
            trace!(
                "Received message ack for message id: {:?}",
                message_ack.message_id
            );
            match &mut unacked_message.unacked_message {
                UnackedMessage::Single { .. } => {
                    if message_ack.fragment_id.is_some() {
                        panic!(
                            "Received a message ack for a fragment but message is a single message"
                        )
                    }
                    for sender in &self.ack_senders {
                        sender.send(message_ack.message_id).unwrap();
                    }
                    self.unacked_messages.remove(&message_ack.message_id);
                }
                UnackedMessage::Fragmented(fragment_acks) => {
                    let Some(fragment_id) = message_ack.fragment_id else {
                        panic!("Received a message ack for a single message but message is a fragmented message")
                    };
                    if !fragment_acks[fragment_id as usize].acked {
                        fragment_acks[fragment_id as usize].acked = true;
                        // TODO: use a variable to keep track of this?
                        // all fragments were acked
                        if fragment_acks.iter().all(|f| f.acked) {
                            self.unacked_messages.remove(&message_ack.message_id);
                            for sender in &self.ack_senders {
                                sender.send(message_ack.message_id).unwrap();
                            }
                        }
                    }
                }
            }
        }
    }

    /// Create a new receiver that will receive a message id when a message is acked
    fn subscribe_acks(&mut self) -> Receiver<MessageId> {
        let (sender, receiver) = crossbeam_channel::unbounded();
        self.ack_senders.push(sender);
        receiver
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
    use bytes::Bytes;
    use core::time::Duration;

    use crate::channel::builder::ReliableSettings;
    use crate::packet::message::SingleData;

    use super::*;

    #[test]
    fn test_reliable_sender_internals() {
        let mut sender = ReliableSender::new(
            ReliableSettings {
                rtt_resend_factor: 1.5,
                rtt_resend_min_delay: Duration::from_millis(100),
            },
            Duration::default(),
        );
        sender.current_rtt = Duration::from_millis(100);
        sender.current_time = WrappedTime::new(0);

        // Buffer a new message
        let message1 = Bytes::from("hello");
        sender.buffer_send(message1.clone(), 1.0).unwrap();
        assert_eq!(sender.unacked_messages.len(), 1);
        assert_eq!(sender.next_send_message_id, MessageId(1));
        // Collect the messages to be sent
        let (single, _) = sender.send_packet();
        assert_eq!(single.len(), 1);

        // Advance by a time that is below the resend threshold
        sender.current_time += Duration::from_millis(100);
        let (single, _) = sender.send_packet();
        assert_eq!(single.len(), 0);

        // Advance by a time that is above the resend threshold
        sender.current_time += Duration::from_millis(200);
        let (single, _) = sender.send_packet();
        assert_eq!(single.len(), 1);
        assert_eq!(
            single.front().unwrap(),
            &SendMessage {
                data: SingleData::new(Some(MessageId(0)), message1.clone()).into(),
                // priority is accumulated every time the message is not sent
                priority: 3.0
            }
        );

        // Ack the first message
        sender.receive_ack(&MessageAck {
            message_id: MessageId(0),
            fragment_id: None,
        });
        assert_eq!(sender.unacked_messages.len(), 0);

        // Advance by a time that is above the resend threshold
        sender.current_time += Duration::from_millis(200);
        let (single, _) = sender.send_packet();
        assert_eq!(single.len(), 0);
    }
}
