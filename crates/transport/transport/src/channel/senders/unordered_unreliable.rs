use alloc::collections::VecDeque;
use alloc::vec::Vec;
use bevy_time::{Real, Time, Timer, TimerMode};
use core::time::Duration;

use crate::channel::registry::{ChannelId, ChannelKind};
use crate::channel::senders::fragment_sender::FragmentSender;
use crate::channel::senders::{
    ChannelSend, PendingSendMessage, SendFlushOutcome, commit_unreliable_candidate,
    is_ready_to_send,
};
use crate::packet::compression::{CompressionConfig, CompressionScratch};
use crate::packet::message::{
    MessageAck, MessageData, MessageId, SendCandidate, SendMessage, SendMessageKey, SingleData,
};
use bytes::Bytes;
use lightyear_link::LinkStats;

/// A sender that simply sends the messages without checking if they were received
/// Does not include any ordering information
#[derive(Debug)]
pub struct UnorderedUnreliableSender {
    /// list of single messages that we want to fit into packets and send
    single_messages_to_send: VecDeque<PendingSendMessage>,
    /// list of fragmented messages that we want to fit into packets and send
    fragmented_messages_to_send: VecDeque<PendingSendMessage>,
    /// Fragmented messages need an id (so they can be reconstructed), this keeps track
    /// of the next id to use
    next_send_fragmented_message_id: MessageId,
    /// Used to split a message into fragments if the message is too big
    fragment_sender: FragmentSender,
    /// Internal timer to determine if the channel is ready to send messages
    timer: Option<Timer>,
    retry_unsent_messages: bool,
}

impl UnorderedUnreliableSender {
    pub(crate) fn new(send_frequency: Duration, retry_unsent_messages: bool) -> Self {
        let timer = if send_frequency == Duration::default() {
            None
        } else {
            Some(Timer::new(send_frequency, TimerMode::Repeating))
        };
        Self {
            single_messages_to_send: VecDeque::new(),
            fragmented_messages_to_send: VecDeque::new(),
            next_send_fragmented_message_id: MessageId::default(),
            fragment_sender: FragmentSender::new(),
            timer,
            retry_unsent_messages,
        }
    }
}

impl ChannelSend for UnorderedUnreliableSender {
    fn set_fragment_size(&mut self, fragment_size: usize) {
        self.fragment_sender.set_fragment_size(fragment_size);
    }

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
        compression_scratch: &mut CompressionScratch,
    ) -> Option<MessageId> {
        if message.len() > self.fragment_sender.fragment_size {
            for fragment in self.fragment_sender.build_fragments_for_message(
                self.next_send_fragmented_message_id,
                message,
                compression,
                compression_scratch,
            ) {
                self.fragmented_messages_to_send
                    .push_back(PendingSendMessage::new(SendMessage {
                        data: MessageData::Fragment(fragment),
                        priority,
                    }));
            }
            self.next_send_fragmented_message_id += 1;
            Some(self.next_send_fragmented_message_id - 1)
        } else {
            let single_data = SingleData::new(None, message);
            self.single_messages_to_send
                .push_back(PendingSendMessage::new(SendMessage {
                    data: MessageData::Single(single_data),
                    priority,
                }));
            None
        }
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
        debug_assert!(committed, "invalid unreliable send candidate");
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

    fn receive_ack(&mut self, _: &MessageAck) {}
}

#[cfg(test)]
mod tests {
    use alloc::vec;

    use super::*;

    struct TestChannel;

    #[test]
    fn candidates_remain_pending_until_committed() {
        let mut sender = UnorderedUnreliableSender::new(Duration::default(), true);
        sender.buffer_send(
            Bytes::from_static(b"pending"),
            1.0,
            CompressionConfig::DISABLED,
            &mut CompressionScratch::default(),
        );

        let mut candidates = Vec::new();
        sender.collect_send_candidates(ChannelKind::of::<TestChannel>(), 0, &mut candidates);
        assert_eq!(candidates.len(), 1);

        // A rejected staged packet calls finish without committing and must not consume data.
        sender.finish_send(SendFlushOutcome::BandwidthLimited);
        candidates.clear();
        sender.collect_send_candidates(ChannelKind::of::<TestChannel>(), 0, &mut candidates);
        assert_eq!(candidates.len(), 1);

        sender.commit_send(candidates[0].key, Duration::default());
        sender.finish_send(SendFlushOutcome::Complete);
        candidates.clear();
        sender.collect_send_candidates(ChannelKind::of::<TestChannel>(), 0, &mut candidates);
        assert!(candidates.is_empty());
    }

    #[test]
    fn retry_unsent_policy_only_discards_after_bandwidth_limit() {
        let mut sender = UnorderedUnreliableSender::new(Duration::default(), false);
        sender.buffer_send(
            Bytes::from_static(b"stale"),
            1.0,
            CompressionConfig::DISABLED,
            &mut CompressionScratch::default(),
        );

        let mut candidates = Vec::new();
        sender.collect_send_candidates(ChannelKind::of::<TestChannel>(), 0, &mut candidates);
        assert_eq!(candidates.len(), 1);

        sender.finish_send(SendFlushOutcome::StagingFailed);
        candidates.clear();
        sender.collect_send_candidates(ChannelKind::of::<TestChannel>(), 0, &mut candidates);
        assert_eq!(candidates.len(), 1);

        sender.finish_send(SendFlushOutcome::BandwidthLimited);
        candidates.clear();
        sender.collect_send_candidates(ChannelKind::of::<TestChannel>(), 0, &mut candidates);
        assert!(candidates.is_empty());
    }

    #[test]
    fn discard_policy_finishes_started_fragmented_messages() {
        let mut wholly_unsent = UnorderedUnreliableSender::new(Duration::default(), false);
        let message_len = wholly_unsent.fragment_sender.fragment_size * 2 + 1;
        wholly_unsent.buffer_send(
            Bytes::from(vec![0; message_len]),
            1.0,
            CompressionConfig::DISABLED,
            &mut CompressionScratch::default(),
        );

        let mut candidates = Vec::new();
        wholly_unsent.collect_send_candidates(ChannelKind::of::<TestChannel>(), 0, &mut candidates);
        assert!(candidates.len() > 1);

        wholly_unsent.finish_send(SendFlushOutcome::BandwidthLimited);
        candidates.clear();
        wholly_unsent.collect_send_candidates(ChannelKind::of::<TestChannel>(), 0, &mut candidates);
        assert!(candidates.is_empty());

        let mut partially_sent = UnorderedUnreliableSender::new(Duration::default(), false);
        partially_sent.buffer_send(
            Bytes::from(vec![0; message_len]),
            1.0,
            CompressionConfig::DISABLED,
            &mut CompressionScratch::default(),
        );
        partially_sent.collect_send_candidates(
            ChannelKind::of::<TestChannel>(),
            0,
            &mut candidates,
        );
        let fragment_count = candidates.len();

        partially_sent.commit_send(candidates[0].key, Duration::default());
        partially_sent.finish_send(SendFlushOutcome::BandwidthLimited);
        candidates.clear();
        partially_sent.collect_send_candidates(
            ChannelKind::of::<TestChannel>(),
            0,
            &mut candidates,
        );
        assert_eq!(candidates.len(), fragment_count - 1);
    }

    #[test]
    fn bandwidth_limit_does_not_discard_messages_before_send_frequency() {
        let mut sender = UnorderedUnreliableSender::new(Duration::from_secs(1), false);
        sender.buffer_send(
            Bytes::from_static(b"not-yet-eligible"),
            1.0,
            CompressionConfig::DISABLED,
            &mut CompressionScratch::default(),
        );

        let mut candidates = Vec::new();
        sender.collect_send_candidates(ChannelKind::of::<TestChannel>(), 0, &mut candidates);
        assert!(candidates.is_empty());
        sender.finish_send(SendFlushOutcome::BandwidthLimited);

        let mut real = Time::<Real>::default();
        real.advance_by(Duration::from_secs(1));
        sender.update(&real, &LinkStats::default());
        sender.collect_send_candidates(ChannelKind::of::<TestChannel>(), 0, &mut candidates);
        assert_eq!(candidates.len(), 1);
    }
}
