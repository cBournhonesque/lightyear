use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use bevy_time::{Real, Time, Timer, TimerMode};

use crate::channel::builder::ReliableSettings;
use crate::channel::registry::{ChannelId, ChannelKind};
use crate::channel::senders::fragment_sender::FragmentSender;
use crate::channel::senders::{ChannelSend, SendFlushOutcome};
use crate::packet::compression::CompressionConfig;
use crate::packet::message::{
    FragmentData, MessageAck, MessageId, SendCandidate, SendMessage, SendMessageKey, SingleData,
};
use bytes::Bytes;
use core::time::Duration;
use lightyear_link::LinkStats;
#[allow(unused_imports)]
use tracing::{info, trace};

#[derive(Debug)]
pub struct FragmentAck {
    data: FragmentData,
    acked: bool,
    last_sent: Option<Duration>,
}

/// A message that has not been acked yet
#[derive(Debug)]
pub enum UnackedMessage {
    Single {
        bytes: Bytes,
        /// If None: this packet has never been sent before
        /// else: the last instant when this packet was sent
        last_sent: Option<Duration>,
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

    /// Used to split a message into fragments if the message is too big
    fragment_sender: FragmentSender,
    current_rtt: Duration,
    current_time: Duration,
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
            fragment_sender: FragmentSender::new(),
            current_rtt: Duration::default(),
            current_time: Duration::default(),
            timer,
            priority_multiplier: 1.0,
        }
    }
}

impl ChannelSend for ReliableSender {
    fn update(&mut self, real_time: &Time<Real>, link_stats: &LinkStats) {
        self.current_time = real_time.elapsed();
        self.current_rtt = link_stats.rtt;
        if let Some(timer) = &mut self.timer {
            timer.tick(real_time.delta());
            self.priority_multiplier =
                timer.duration().as_nanos() as f32 / real_time.delta().as_nanos() as f32;
            trace!(
                ?timer,
                "Priority multiplier for reliable sender channel: {:?}", self.priority_multiplier
            );
        }
    }

    /// Add a new message to the buffer of messages to be sent.
    /// This is a client-facing function, to be called when you want to send a message
    fn buffer_send(
        &mut self,
        message: Bytes,
        priority: f32,
        compression: CompressionConfig,
    ) -> Option<MessageId> {
        let message_id = self.next_send_message_id;
        let unacked_message = if message.len() > self.fragment_sender.fragment_size {
            let fragments =
                self.fragment_sender
                    .build_fragments_for_message(message_id, message, compression);
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
        Some(message_id)
    }

    fn collect_send_candidates(
        &mut self,
        channel_kind: ChannelKind,
        channel_id: ChannelId,
        output: &mut Vec<SendCandidate>,
    ) {
        if self.timer.as_ref().is_some_and(|t| !t.is_finished()) {
            return;
        }

        // Collect the list of messages that need to be sent
        // Either because they have never been sent, or because they need to be resent

        // resend delay is based on the rtt
        let resend_delay = self.reliable_settings.resend_delay(self.current_rtt);
        let should_send = |last_sent: &Option<Duration>| -> bool {
            match last_sent {
                // send if the message has never been sent
                None => true,
                // or if we sent it a while back but didn't get an ack
                Some(last_sent) => {
                    resend_delay != Duration::default()
                        && self.current_time.saturating_sub(*last_sent) > resend_delay
                }
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
                UnackedMessage::Single { bytes, last_sent } => {
                    if should_send(last_sent) {
                        trace!(?last_sent, ?self.current_time, "Should send message {:?}", message_id);
                        output.push(SendCandidate::new(
                            channel_kind,
                            channel_id,
                            SendMessageKey::ReliableSingle(*message_id),
                            u64::from(message_id.0),
                            SendMessage {
                                data: SingleData::new(Some(*message_id), bytes.clone()).into(),
                                priority: unacked_message_with_priority.accumulated_priority,
                            },
                        ));
                    }
                }
                UnackedMessage::Fragmented(fragment_acks) => {
                    // only send the fragments that haven't been acked and should be resent
                    fragment_acks
                        .iter_mut()
                        .filter(|f| !f.acked && should_send(&f.last_sent))
                        .for_each(|f| {
                            output.push(SendCandidate::new(
                                channel_kind,
                                channel_id,
                                SendMessageKey::ReliableFragment(*message_id, f.data.fragment_id),
                                u64::from(message_id.0),
                                SendMessage {
                                    data: f.data.clone().into(),
                                    priority: unacked_message_with_priority.accumulated_priority,
                                },
                            ));
                        })
                }
            }
        }
    }

    fn commit_send(&mut self, key: SendMessageKey, sent_at: Duration) {
        match key {
            SendMessageKey::ReliableSingle(message_id) => {
                let Some(message) = self.unacked_messages.get_mut(&message_id) else {
                    debug_assert!(false, "missing reliable message during send commit");
                    return;
                };
                let UnackedMessage::Single { last_sent, .. } = &mut message.unacked_message else {
                    debug_assert!(false, "reliable send key did not match message shape");
                    return;
                };
                *last_sent = Some(sent_at);
            }
            SendMessageKey::ReliableFragment(message_id, fragment_id) => {
                let Some(message) = self.unacked_messages.get_mut(&message_id) else {
                    debug_assert!(
                        false,
                        "missing fragmented reliable message during send commit"
                    );
                    return;
                };
                let UnackedMessage::Fragmented(fragments) = &mut message.unacked_message else {
                    debug_assert!(false, "reliable fragment key did not match message shape");
                    return;
                };
                let Some(fragment) = fragments.get_mut(fragment_id.0 as usize) else {
                    debug_assert!(false, "missing reliable fragment during send commit");
                    return;
                };
                fragment.last_sent = Some(sent_at);
            }
            _ => debug_assert!(false, "unreliable key committed to reliable sender"),
        }
    }

    fn finish_send(&mut self, _: SendFlushOutcome) {}

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
                    self.unacked_messages.remove(&message_ack.message_id);
                }
                UnackedMessage::Fragmented(fragment_acks) => {
                    let Some(fragment_id) = message_ack.fragment_id else {
                        panic!(
                            "Received a message ack for a single message but message is a fragmented message"
                        )
                    };
                    if !fragment_acks[fragment_id.0 as usize].acked {
                        fragment_acks[fragment_id.0 as usize].acked = true;
                        // TODO: use a variable to keep track of this?
                        // all fragments were acked
                        if fragment_acks.iter().all(|f| f.acked) {
                            self.unacked_messages.remove(&message_ack.message_id);
                        }
                    }
                }
            }
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

    struct TestChannel;

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
        sender.current_time = Duration::default();

        // Buffer a new message
        let message1 = Bytes::from("hello");
        sender
            .buffer_send(message1.clone(), 1.0, CompressionConfig::DISABLED)
            .unwrap();
        assert_eq!(sender.unacked_messages.len(), 1);
        assert_eq!(sender.next_send_message_id, MessageId(1));
        // Collect the messages to be sent
        let mut candidates = Vec::new();
        sender.collect_send_candidates(ChannelKind::of::<TestChannel>(), 0, &mut candidates);
        assert_eq!(candidates.len(), 1);
        assert!(matches!(
            sender.unacked_messages[&MessageId(0)].unacked_message,
            UnackedMessage::Single {
                last_sent: None,
                ..
            }
        ));
        sender.commit_send(candidates[0].key, sender.current_time);

        // Advance by a time that is below the resend threshold
        sender.current_time += Duration::from_millis(100);
        candidates.clear();
        sender.collect_send_candidates(ChannelKind::of::<TestChannel>(), 0, &mut candidates);
        assert!(candidates.is_empty());

        // Advance by a time that is above the resend threshold
        sender.current_time += Duration::from_millis(200);
        candidates.clear();
        sender.collect_send_candidates(ChannelKind::of::<TestChannel>(), 0, &mut candidates);
        assert_eq!(candidates.len(), 1);
        assert_eq!(
            candidates[0].message,
            SendMessage {
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
        candidates.clear();
        sender.collect_send_candidates(ChannelKind::of::<TestChannel>(), 0, &mut candidates);
        assert!(candidates.is_empty());
    }
}
