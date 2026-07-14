use crate::channel::registry::{ChannelId, ChannelKind};
use crate::channel::senders::fragment_sender::FragmentSender;
use crate::channel::senders::{
    ChannelSend, PendingSendMessage, SendFlushOutcome, commit_unreliable_candidate,
    is_ready_to_send,
};
use crate::packet::compression::CompressionConfig;
use crate::packet::message::{
    MessageAck, MessageData, MessageId, SendCandidate, SendMessage, SendMessageKey, SingleData,
};
use alloc::collections::VecDeque;
use alloc::vec::Vec;
use bevy_time::{Real, Time, Timer, TimerMode};
use bytes::Bytes;
use core::time::Duration;
use lightyear_link::LinkStats;

/// A sender that simply sends the messages without checking if they were received
/// Same as UnorderedUnreliableSender, but includes ordering information (MessageId)
#[derive(Debug)]
pub struct SequencedUnreliableSender {
    /// list of single messages that we want to fit into packets and send
    single_messages_to_send: VecDeque<PendingSendMessage>,
    /// list of fragmented messages that we want to fit into packets and send
    fragmented_messages_to_send: VecDeque<PendingSendMessage>,

    /// Message id to use for the next message to be sent
    next_send_message_id: MessageId,
    /// Used to split a message into fragments if the message is too big
    fragment_sender: FragmentSender,
    /// Internal timer to determine if the channel is ready to send messages
    timer: Option<Timer>,
    retry_unsent_messages: bool,
}

impl SequencedUnreliableSender {
    pub(crate) fn new(send_frequency: Duration, retry_unsent_messages: bool) -> Self {
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
            timer,
            retry_unsent_messages,
        }
    }
}

impl ChannelSend for SequencedUnreliableSender {
    fn update(&mut self, real_time: &Time<Real>, _: &LinkStats) {
        if let Some(timer) = &mut self.timer {
            timer.tick(real_time.delta());
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
        if message.len() > self.fragment_sender.fragment_size {
            for fragment in
                self.fragment_sender
                    .build_fragments_for_message(message_id, message, compression)
            {
                self.fragmented_messages_to_send
                    .push_back(PendingSendMessage::new(SendMessage {
                        data: MessageData::Fragment(fragment),
                        priority,
                    }));
            }
        } else {
            let single_data = SingleData::new(Some(message_id), message);
            self.single_messages_to_send
                .push_back(PendingSendMessage::new(SendMessage {
                    data: MessageData::Single(single_data),
                    priority,
                }));
        }
        self.next_send_message_id += 1;
        Some(message_id)
    }

    fn collect_send_candidates(
        &mut self,
        channel_kind: ChannelKind,
        channel_id: ChannelId,
        output: &mut Vec<SendCandidate>,
    ) {
        if !is_ready_to_send(self.timer.as_ref()) {
            return;
        }
        output.extend(
            self.single_messages_to_send
                .iter()
                .enumerate()
                .filter(|(_, pending)| !pending.committed)
                .map(|(index, pending)| {
                    SendCandidate::new(
                        channel_kind,
                        channel_id,
                        SendMessageKey::UnreliableSingle(index),
                        pending.message.clone(),
                    )
                }),
        );
        output.extend(
            self.fragmented_messages_to_send
                .iter()
                .enumerate()
                .filter(|(_, pending)| !pending.committed)
                .map(|(index, pending)| {
                    SendCandidate::new(
                        channel_kind,
                        channel_id,
                        SendMessageKey::UnreliableFragment(index),
                        pending.message.clone(),
                    )
                }),
        );
    }

    fn commit_send(&mut self, key: SendMessageKey, _: Duration) {
        let committed = commit_unreliable_candidate(
            &mut self.single_messages_to_send,
            &mut self.fragmented_messages_to_send,
            key,
        );
        debug_assert!(committed, "invalid sequenced-unreliable send candidate");
    }

    fn finish_send(&mut self, outcome: SendFlushOutcome) {
        let discard_unsent = is_ready_to_send(self.timer.as_ref())
            && !self.retry_unsent_messages
            && outcome == SendFlushOutcome::BandwidthLimited;
        if discard_unsent {
            self.single_messages_to_send.clear();
            self.fragmented_messages_to_send
                .retain(|pending| !pending.committed && pending.fragment_started);
        } else {
            self.single_messages_to_send
                .retain(|pending| !pending.committed);
            self.fragmented_messages_to_send
                .retain(|pending| !pending.committed);
        }
    }

    fn receive_ack(&mut self, _message_ack: &MessageAck) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    struct TestChannel;
    #[test]
    fn test_sequenced_unreliable_sender_internals() {
        let mut sender = SequencedUnreliableSender::new(Duration::from_secs(1), true);
        assert!(sender.timer.as_ref().is_some_and(|t| !t.is_finished()));

        sender
            .buffer_send(Bytes::from("hello"), 1.0, CompressionConfig::DISABLED)
            .unwrap();

        // we do not send because we didn't reach the timer
        let mut candidates = Vec::new();
        sender.collect_send_candidates(ChannelKind::of::<TestChannel>(), 0, &mut candidates);
        assert!(candidates.is_empty());

        // update with a delta of 1 second
        let mut real = Time::<Real>::default();
        real.advance_by(Duration::from_secs(1));
        let link_stats = LinkStats::default();
        sender.update(&real, &link_stats);

        // this time, we send the packet
        sender.collect_send_candidates(ChannelKind::of::<TestChannel>(), 0, &mut candidates);
        assert_eq!(candidates.len(), 1);
    }
}
