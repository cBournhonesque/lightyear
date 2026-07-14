use alloc::collections::VecDeque;
use alloc::vec::Vec;
use bevy_time::{Real, Time, Timer, TimerMode};
use core::time::Duration;

use crate::channel::registry::{ChannelId, ChannelKind};
use crate::channel::senders::fragment_ack_receiver::FragmentAckReceiver;
use crate::channel::senders::fragment_sender::FragmentSender;
use crate::channel::senders::{
    ChannelSend, PendingSendMessage, SendFlushOutcome, commit_unreliable_candidate,
    is_ready_to_send,
};
use crate::packet::compression::CompressionConfig;
use crate::packet::message::{
    MessageAck, MessageData, MessageId, SendCandidate, SendMessage, SendMessageKey, SingleData,
};
use bytes::Bytes;
use lightyear_link::LinkStats;

const DISCARD_AFTER: Duration = Duration::from_millis(3000);

/// A sender that simply sends the messages without applying any reliability or unordered
/// Same as UnorderedUnreliableSender, but includes a message id to each message,
/// Which can let us track if a message was acked
#[derive(Debug)]
pub struct UnorderedUnreliableWithAcksSender {
    /// list of single messages that we want to fit into packets and send
    single_messages_to_send: VecDeque<PendingSendMessage>,
    /// list of fragmented messages that we want to fit into packets and send
    fragmented_messages_to_send: VecDeque<PendingSendMessage>,
    /// Message id to use for the next message to be sent
    next_send_message_id: MessageId,
    /// Used to split a message into fragments if the message is too big
    fragment_sender: FragmentSender,
    /// Keep track of which fragments were acked, so we can know when the entire fragment message
    /// was acked
    fragment_ack_receiver: FragmentAckReceiver,
    /// Internal timer to determine if the channel is ready to send messages
    timer: Option<Timer>,
    retry_unsent_messages: bool,
}

impl UnorderedUnreliableWithAcksSender {
    pub(crate) fn new(send_frequency: Duration, retry_unsent_messages: bool) -> Self {
        let timer = if send_frequency == Duration::default() {
            None
        } else {
            Some(Timer::new(send_frequency, TimerMode::Repeating))
        };
        Self {
            single_messages_to_send: VecDeque::new(),
            fragmented_messages_to_send: VecDeque::new(),
            next_send_message_id: MessageId::default(),
            fragment_sender: FragmentSender::new(),
            fragment_ack_receiver: FragmentAckReceiver::new(),
            timer,
            retry_unsent_messages,
        }
    }
}

impl ChannelSend for UnorderedUnreliableWithAcksSender {
    fn update(&mut self, real_time: &Time<Real>, _: &LinkStats) {
        self.fragment_ack_receiver
            .cleanup(real_time.elapsed().saturating_sub(DISCARD_AFTER));
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
            let fragments =
                self.fragment_sender
                    .build_fragments_for_message(message_id, message, compression);
            self.fragment_ack_receiver
                .add_new_fragment_to_wait_for(message_id, fragments.len());
            for fragment in fragments {
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
        debug_assert!(committed, "invalid unreliable-with-acks send candidate");
    }

    fn finish_send(&mut self, outcome: SendFlushOutcome) {
        let discard_unsent = is_ready_to_send(self.timer.as_ref())
            && !self.retry_unsent_messages
            && outcome == SendFlushOutcome::BandwidthLimited;
        if discard_unsent {
            for pending in self
                .fragmented_messages_to_send
                .iter()
                .filter(|pending| !pending.committed && !pending.fragment_started)
            {
                if let MessageData::Fragment(fragment) = &pending.message.data
                    && fragment.fragment_id.0 == 0
                {
                    self.fragment_ack_receiver
                        .discard_message(fragment.message_id);
                }
            }
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

    /// Notify any subscribers that a message was acked
    fn receive_ack(&mut self, ack: &MessageAck) {
        if let Some(fragment_index) = ack.fragment_id {
            self.fragment_ack_receiver
                .receive_fragment_ack(ack.message_id, fragment_index, None);
        }
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec;

    use super::*;

    struct TestChannel;

    #[test]
    fn discarding_wholly_unsent_fragments_clears_ack_tracking() {
        let mut sender = UnorderedUnreliableWithAcksSender::new(Duration::default(), false);
        let message_len = sender.fragment_sender.fragment_size * 2 + 1;
        sender.buffer_send(
            Bytes::from(vec![0; message_len]),
            1.0,
            CompressionConfig::DISABLED,
        );

        let mut candidates = Vec::new();
        sender.collect_send_candidates(ChannelKind::of::<TestChannel>(), 0, &mut candidates);
        assert!(candidates.len() > 1);

        sender.finish_send(SendFlushOutcome::BandwidthLimited);
        assert!(sender.fragmented_messages_to_send.is_empty());
        assert_eq!(sender.fragment_ack_receiver, FragmentAckReceiver::new());
    }
}
