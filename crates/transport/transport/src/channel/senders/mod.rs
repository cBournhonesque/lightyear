use crate::channel::registry::{ChannelId, ChannelKind};
use crate::channel::senders::reliable::ReliableSender;
use crate::channel::senders::sequenced_unreliable::SequencedUnreliableSender;
use crate::channel::senders::unordered_unreliable::UnorderedUnreliableSender;
use crate::channel::senders::unordered_unreliable_with_acks::UnorderedUnreliableWithAcksSender;
use crate::packet::compression::CompressionConfig;
use crate::packet::message::{
    MessageAck, MessageData, MessageId, SendCandidate, SendMessage, SendMessageKey,
};
use crate::prelude::{ChannelMode, ChannelSettings};
use alloc::collections::VecDeque;
use alloc::vec::Vec;
use bevy_time::{Real, Time, Timer};
use bytes::Bytes;
use core::time::Duration;
use enum_dispatch::enum_dispatch;
use lightyear_link::LinkStats;

pub(crate) mod fragment_ack_receiver;
pub(crate) mod fragment_sender;
pub(crate) mod reliable;
pub(crate) mod sequenced_unreliable;
pub(crate) mod unordered_unreliable;
pub(crate) mod unordered_unreliable_with_acks;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SendFlushOutcome {
    Complete,
    BandwidthLimited,
    StagingFailed,
}

// TODO: separate trait into multiple traits
// - buffer send should be public
// - all other methods should be private
/// A trait for sending messages to a channel.
/// A channel is a buffer over packets to be able to add ordering/reliability
#[enum_dispatch]
pub(crate) trait ChannelSend {
    /// Configures the fixed fragment payload size derived from the link's stable minimum MTU.
    fn set_fragment_size(&mut self, fragment_size: usize);

    /// Bookkeeping for the channel
    fn update(&mut self, real_time: &Time<Real>, link_stats: &LinkStats);

    /// Queues a message to be transmitted.
    /// The priority of the message needs to be specified
    ///
    /// Returns the MessageId of the message that was queued, if there is one
    fn buffer_send(
        &mut self,
        message: Bytes,
        priority: f32,
        compression: CompressionConfig,
    ) -> Option<MessageId>;

    /// Append cheap snapshots of the messages which are currently eligible for packet staging.
    ///
    /// This must not remove messages or mark them as sent. Packet building and bandwidth admission
    /// are speculative until [`Self::commit_send`] is called.
    fn collect_send_candidates(
        &mut self,
        channel_kind: ChannelKind,
        channel_id: ChannelId,
        output: &mut Vec<SendCandidate>,
    );

    /// Commit one candidate after the packet containing it has entered `Link.send`.
    fn commit_send(&mut self, key: SendMessageKey, sent_at: Duration);

    /// Finish a send flush and apply the channel's local admission policy.
    fn finish_send(&mut self, outcome: SendFlushOutcome);

    /// Called when we receive acknowledgement that a Message has been received
    fn receive_ack(&mut self, message_ack: &MessageAck);
}

/// Channel-owned state for an unreliable message during speculative packet staging.
///
/// A completed flush removes admitted entries and either retains or discards unadmitted entries
/// according to [`ChannelSettings::retry_unsent_messages`].
#[derive(Debug)]
pub(super) struct PendingSendMessage {
    pub(super) message: SendMessage,
    pub(super) committed: bool,
    /// Once one fragment is admitted, retain the rest even on a discard-unsent channel so the
    /// local sender does not guarantee an incomplete fragmented message.
    pub(super) fragment_started: bool,
}

impl PendingSendMessage {
    pub(super) fn new(message: SendMessage) -> Self {
        Self {
            message,
            committed: false,
            fragment_started: false,
        }
    }

    pub(super) fn fragment_message_id(&self) -> Option<MessageId> {
        let MessageData::Fragment(fragment) = &self.message.data else {
            return None;
        };
        Some(fragment.message_id)
    }
}

pub(super) fn is_ready_to_send(timer: Option<&Timer>) -> bool {
    timer.is_none_or(Timer::is_finished)
}

pub(super) fn commit_unreliable_candidate(
    single_messages: &mut VecDeque<PendingSendMessage>,
    fragmented_messages: &mut VecDeque<PendingSendMessage>,
    key: SendMessageKey,
) -> bool {
    match key {
        SendMessageKey::UnreliableSingle(index) => {
            let Some(pending) = single_messages.get_mut(index) else {
                return false;
            };
            pending.committed = true;
        }
        SendMessageKey::UnreliableFragment(index) => {
            let Some(pending) = fragmented_messages.get_mut(index) else {
                return false;
            };
            let message_id = pending
                .fragment_message_id()
                .expect("fragment queue must contain fragment data");
            let mark_message_started = !pending.fragment_started;
            pending.committed = true;

            if mark_message_started {
                for pending in fragmented_messages {
                    if pending.fragment_message_id() == Some(message_id) {
                        pending.fragment_started = true;
                    }
                }
            }
        }
        _ => return false,
    }
    true
}

/// Enum dispatch lets us derive ChannelSend on each enum variant
#[derive(Debug)]
#[enum_dispatch(ChannelSend)]
pub enum ChannelSenderEnum {
    UnorderedUnreliableWithAcks(UnorderedUnreliableWithAcksSender),
    UnorderedUnreliable(UnorderedUnreliableSender),
    SequencedUnreliable(SequencedUnreliableSender),
    Reliable(ReliableSender),
}

impl From<&ChannelSettings> for ChannelSenderEnum {
    fn from(settings: &ChannelSettings) -> Self {
        match settings.mode {
            ChannelMode::UnorderedUnreliableWithAcks => UnorderedUnreliableWithAcksSender::new(
                settings.send_frequency,
                settings.retry_unsent_messages,
            )
            .into(),
            ChannelMode::UnorderedUnreliable => UnorderedUnreliableSender::new(
                settings.send_frequency,
                settings.retry_unsent_messages,
            )
            .into(),
            ChannelMode::SequencedUnreliable => SequencedUnreliableSender::new(
                settings.send_frequency,
                settings.retry_unsent_messages,
            )
            .into(),
            ChannelMode::UnorderedReliable(reliable_settings) => {
                ReliableSender::new(reliable_settings, settings.send_frequency).into()
            }
            ChannelMode::SequencedReliable(reliable_settings) => {
                ReliableSender::new(reliable_settings, settings.send_frequency).into()
            }
            ChannelMode::OrderedReliable(reliable_settings) => {
                ReliableSender::new(reliable_settings, settings.send_frequency).into()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channel::builder::ReliableSettings;

    struct TestChannel;

    #[test]
    fn retry_unsent_messages_defaults_to_true() {
        assert!(ChannelSettings::default().retry_unsent_messages);
    }

    #[test]
    fn channel_setting_discards_unadmitted_unreliable_messages() {
        for mode in [
            ChannelMode::UnorderedUnreliable,
            ChannelMode::SequencedUnreliable,
            ChannelMode::UnorderedUnreliableWithAcks,
        ] {
            let settings = ChannelSettings {
                mode,
                retry_unsent_messages: false,
                ..Default::default()
            };
            let mut sender = ChannelSenderEnum::from(&settings);
            sender.buffer_send(
                Bytes::from_static(b"stale"),
                1.0,
                CompressionConfig::DISABLED,
            );

            let mut candidates = Vec::new();
            sender.collect_send_candidates(ChannelKind::of::<TestChannel>(), 0, &mut candidates);
            assert_eq!(candidates.len(), 1);
            sender.finish_send(SendFlushOutcome::BandwidthLimited);

            candidates.clear();
            sender.collect_send_candidates(ChannelKind::of::<TestChannel>(), 0, &mut candidates);
            assert!(candidates.is_empty());
        }
    }

    #[test]
    fn reliable_messages_ignore_the_unreliable_discard_policy() {
        let settings = ChannelSettings {
            mode: ChannelMode::UnorderedReliable(ReliableSettings::default()),
            retry_unsent_messages: false,
            ..Default::default()
        };
        let mut sender = ChannelSenderEnum::from(&settings);
        sender.buffer_send(
            Bytes::from_static(b"reliable"),
            1.0,
            CompressionConfig::DISABLED,
        );

        let mut candidates = Vec::new();
        sender.collect_send_candidates(ChannelKind::of::<TestChannel>(), 0, &mut candidates);
        assert_eq!(candidates.len(), 1);
        sender.finish_send(SendFlushOutcome::BandwidthLimited);

        candidates.clear();
        sender.collect_send_candidates(ChannelKind::of::<TestChannel>(), 0, &mut candidates);
        assert_eq!(candidates.len(), 1);
    }
}
