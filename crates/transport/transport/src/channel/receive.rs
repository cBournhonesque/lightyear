//! Channel receive state and delivery policies.

use alloc::collections::VecDeque;
use bevy_platform::collections::{HashMap, HashSet};
use bytes::Bytes;
use core::time::Duration;
use lightyear_core::tick::Tick;

use crate::channel::builder::{ChannelMode, ChannelSettings};
use crate::channel::registry::ChannelKind;
use crate::packet::message::{MessageData, MessageId, ReceiveMessage};

use crate::channel::fragments::FragmentReceiver;

type Result<T> = core::result::Result<T, ChannelReceiveError>;
#[derive(thiserror::Error, Debug)]
pub enum ChannelReceiveError {
    #[error("A message was received without a message ID")]
    MissingMessageId,
    #[error("fragmented message declared an invalid fragment count: {num_fragments}")]
    InvalidFragmentCount { num_fragments: u64 },
    #[error("fragment index {fragment_index} is outside fragment count {num_fragments}")]
    InvalidFragmentIndex {
        fragment_index: u64,
        num_fragments: u64,
    },
    #[error("fragment count changed while reassembling message: expected {expected}, got {actual}")]
    FragmentCountMismatch { expected: usize, actual: usize },
    #[error("fragmented message size overflows the local address space")]
    FragmentedMessageSizeOverflow,
    #[error(
        "fragment compression changed while reassembling message: expected {expected}, got {actual}"
    )]
    FragmentCompressionMismatch {
        expected: &'static str,
        actual: &'static str,
    },
    #[error("fragmented message completed before receiving compression metadata")]
    MissingFragmentCompression,
    #[error("fragment uses unsupported compression: {compression}")]
    UnsupportedFragmentCompression { compression: &'static str },
    #[error("compressed fragment payload could not be decompressed")]
    FragmentDecompressionFailed,
    #[error("decompressed fragment payload size {actual} exceeds configured limit {limit}")]
    FragmentDecompressedPayloadTooLarge { actual: usize, limit: usize },
    #[error("non-final fragment has size {actual}, expected {expected}")]
    InvalidNonFinalFragmentSize { actual: usize, expected: usize },
    #[error("final fragment has size {actual}, maximum {max}")]
    InvalidFinalFragmentSize { actual: usize, max: usize },
}
const DISCARD_AFTER: Duration = Duration::from_millis(3000);

type ReceivedMessage = (Tick, Bytes);

/// The receiving half of one registered transport channel.
///
/// Fragment reassembly and time bookkeeping are shared by every delivery mode. Only the policy
/// for accepting and exposing completed messages varies through the private state enum.
#[derive(Debug)]
pub struct ChannelReceive {
    channel_kind: ChannelKind,
    fragments: FragmentReceiver,
    current_time: Duration,
    state: RecvState,
}

#[derive(Debug)]
enum RecvState {
    UnreliableUnordered {
        ready: VecDeque<ReceivedMessage>,
    },
    UnreliableSequenced {
        ready: Option<(Tick, Bytes, MessageId)>,
        newest_seen: Option<MessageId>,
        newest_completed: Option<MessageId>,
    },
    ReliableUnordered {
        /// Oldest message ID not yet covered by the contiguous received prefix.
        pending: MessageId,
        /// Completed messages in completion order, as required by unordered delivery.
        ready: VecDeque<(MessageId, Tick, Bytes)>,
        /// Exact IDs received at or after `pending`, used for duplicate suppression.
        received: HashSet<MessageId>,
    },
    ReliableOrdered {
        /// Next message ID eligible for delivery.
        pending: MessageId,
        /// Exact completed-message lookup; order is anchored by `pending`, not the map.
        ready: HashMap<MessageId, ReceivedMessage>,
    },
    ReliableSequenced {
        ready: Option<(Tick, Bytes, MessageId)>,
        newest_seen: Option<MessageId>,
        newest_completed: Option<MessageId>,
    },
}

impl ChannelReceive {
    pub(crate) fn new(channel_kind: ChannelKind, settings: &ChannelSettings) -> Self {
        let state = match settings.mode {
            ChannelMode::UnorderedUnreliableWithAcks | ChannelMode::UnorderedUnreliable => {
                RecvState::UnreliableUnordered {
                    ready: VecDeque::new(),
                }
            }
            ChannelMode::SequencedUnreliable => RecvState::UnreliableSequenced {
                ready: None,
                newest_seen: None,
                newest_completed: None,
            },
            ChannelMode::UnorderedReliable(_) => RecvState::ReliableUnordered {
                pending: MessageId::default(),
                ready: VecDeque::new(),
                received: HashSet::default(),
            },
            ChannelMode::OrderedReliable(_) => RecvState::ReliableOrdered {
                pending: MessageId::default(),
                ready: HashMap::default(),
            },
            ChannelMode::SequencedReliable(_) => RecvState::ReliableSequenced {
                ready: None,
                newest_seen: None,
                newest_completed: None,
            },
        };
        Self {
            channel_kind,
            fragments: FragmentReceiver::new(),
            current_time: Duration::default(),
            state,
        }
    }

    /// The type-level channel key associated with this channel.
    pub fn channel_kind(&self) -> ChannelKind {
        self.channel_kind
    }

    pub(crate) fn set_fragment_size(&mut self, fragment_size: usize) {
        self.fragments.set_fragment_size(fragment_size);
    }

    pub(crate) fn update(&mut self, now: Duration) {
        self.current_time = now;
        if self.state.expires_fragments() {
            self.fragments
                .cleanup(self.current_time.saturating_sub(DISCARD_AFTER));
        }
    }

    pub(crate) fn buffer_recv(&mut self, message: ReceiveMessage) -> Result<()> {
        let message_id = message.data.message_id();
        if !self.state.prepare_receive(message_id)? {
            return Ok(());
        }

        let completed = match message.data {
            MessageData::Single(single) => Some((message.remote_sent_tick, single.bytes)),
            MessageData::Fragment(fragment) => self.fragments.receive_fragment(
                fragment,
                message.remote_sent_tick,
                self.state.expires_fragments().then_some(self.current_time),
                message.compression,
            )?,
        };
        if let Some((tick, bytes)) = completed {
            self.state.push_completed(message_id, tick, bytes);
        }
        Ok(())
    }

    /// Removes and returns the next message accepted by the channel's delivery policy.
    pub fn read_message(&mut self) -> Option<(Tick, Bytes, Option<MessageId>)> {
        self.state.read_message()
    }
}

impl RecvState {
    fn expires_fragments(&self) -> bool {
        matches!(
            self,
            Self::UnreliableUnordered { .. } | Self::UnreliableSequenced { .. }
        )
    }

    fn prepare_receive(&mut self, message_id: Option<MessageId>) -> Result<bool> {
        match self {
            Self::UnreliableUnordered { .. } => Ok(true),
            Self::UnreliableSequenced {
                ready, newest_seen, ..
            } => {
                let message_id = message_id.ok_or(ChannelReceiveError::MissingMessageId)?;
                match *newest_seen {
                    Some(newest) if message_id < newest => return Ok(false),
                    Some(newest) if message_id > newest => {
                        *newest_seen = Some(message_id);
                        // A newer sequence invalidates an older completed message even if the
                        // application has not drained it yet.
                        *ready = None;
                    }
                    None => *newest_seen = Some(message_id),
                    Some(_) => {}
                }
                Ok(true)
            }
            Self::ReliableUnordered {
                pending, received, ..
            } => {
                let message_id = message_id.ok_or(ChannelReceiveError::MissingMessageId)?;
                Ok(message_id - *pending >= 0 && !received.contains(&message_id))
            }
            Self::ReliableOrdered { pending, ready } => {
                let message_id = message_id.ok_or(ChannelReceiveError::MissingMessageId)?;
                Ok(message_id - *pending >= 0 && !ready.contains_key(&message_id))
            }
            Self::ReliableSequenced {
                ready,
                newest_seen,
                newest_completed,
            } => {
                let message_id = message_id.ok_or(ChannelReceiveError::MissingMessageId)?;
                // `newest_completed` deliberately outlives `ready`: taking a message for
                // application delivery must not make a later reliable retransmission eligible
                // for delivery again.
                if *newest_completed == Some(message_id) {
                    return Ok(false);
                }
                match *newest_seen {
                    Some(newest) if message_id < newest => return Ok(false),
                    Some(newest) if message_id > newest => {
                        *newest_seen = Some(message_id);
                        // Sequenced delivery exposes only the newest completed message.
                        *ready = None;
                    }
                    None => *newest_seen = Some(message_id),
                    Some(_) => {}
                }
                Ok(true)
            }
        }
    }

    fn push_completed(&mut self, message_id: Option<MessageId>, tick: Tick, bytes: Bytes) {
        match self {
            Self::UnreliableUnordered { ready } => ready.push_back((tick, bytes)),
            Self::UnreliableSequenced {
                ready,
                newest_seen,
                newest_completed,
            }
            | Self::ReliableSequenced {
                ready,
                newest_seen,
                newest_completed,
            } => {
                let message_id = message_id.expect("sequenced messages have ids");
                if *newest_completed == Some(message_id)
                    || newest_seen.is_some_and(|newest| message_id < newest)
                {
                    return;
                }
                *newest_completed = Some(message_id);
                *ready = Some((tick, bytes, message_id));
            }
            Self::ReliableUnordered {
                ready, received, ..
            } => {
                let message_id = message_id.expect("reliable messages have ids");
                received.insert(message_id);
                ready.push_back((message_id, tick, bytes));
            }
            Self::ReliableOrdered { ready, .. } => {
                ready.insert(
                    message_id.expect("reliable messages have ids"),
                    (tick, bytes),
                );
            }
        }
    }

    fn read_message(&mut self) -> Option<(Tick, Bytes, Option<MessageId>)> {
        match self {
            Self::UnreliableUnordered { ready } => {
                ready.pop_front().map(|(tick, bytes)| (tick, bytes, None))
            }
            Self::UnreliableSequenced { ready, .. } | Self::ReliableSequenced { ready, .. } => {
                ready
                    .take()
                    .map(|(tick, bytes, id)| (tick, bytes, Some(id)))
            }
            Self::ReliableUnordered {
                pending,
                ready,
                received,
            } => {
                let (message_id, tick, bytes) = ready.pop_front()?;
                if *pending == message_id {
                    while received.remove(pending) {
                        *pending += 1;
                    }
                }
                Some((tick, bytes, Some(message_id)))
            }
            Self::ReliableOrdered { pending, ready } => {
                let (tick, bytes) = ready.remove(pending)?;
                let message_id = *pending;
                *pending += 1;
                Some((tick, bytes, Some(message_id)))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channel::builder::ReliableSettings;
    use crate::packet::compression::CompressionConfig;
    use crate::packet::message::SingleData;

    struct TestChannel;

    fn message(id: Option<u32>, tick: u32, bytes: &'static [u8]) -> ReceiveMessage {
        ReceiveMessage {
            data: SingleData::new(id.map(MessageId), Bytes::from_static(bytes)).into(),
            remote_sent_tick: Tick(tick),
            compression: CompressionConfig::DISABLED,
        }
    }

    fn channel(mode: ChannelMode) -> ChannelReceive {
        ChannelReceive::new(
            ChannelKind::of::<TestChannel>(),
            &ChannelSettings {
                mode,
                ..ChannelSettings::default()
            },
        )
    }

    #[test]
    fn every_channel_mode_constructs_one_channel_receive() {
        for mode in [
            ChannelMode::UnorderedUnreliable,
            ChannelMode::UnorderedUnreliableWithAcks,
            ChannelMode::SequencedUnreliable,
            ChannelMode::UnorderedReliable(ReliableSettings::default()),
            ChannelMode::OrderedReliable(ReliableSettings::default()),
            ChannelMode::SequencedReliable(ReliableSettings::default()),
        ] {
            assert_eq!(
                channel(mode).channel_kind(),
                ChannelKind::of::<TestChannel>()
            );
        }
    }

    #[test]
    fn ordered_reliable_waits_for_the_missing_sequence() {
        let mut channel = channel(ChannelMode::OrderedReliable(ReliableSettings::default()));
        channel.buffer_recv(message(Some(1), 1, b"one")).unwrap();
        assert_eq!(channel.read_message(), None);
        channel.buffer_recv(message(Some(0), 0, b"zero")).unwrap();
        assert_eq!(channel.read_message().unwrap().2, Some(MessageId(0)));
        assert_eq!(channel.read_message().unwrap().2, Some(MessageId(1)));
    }

    #[test]
    fn ordered_reliable_buffers_across_message_id_rollover() {
        let mut channel = channel(ChannelMode::OrderedReliable(ReliableSettings::default()));
        let RecvState::ReliableOrdered { pending, .. } = &mut channel.state else {
            unreachable!()
        };
        *pending = MessageId(u32::MAX);

        channel.buffer_recv(message(Some(0), 0, b"zero")).unwrap();
        assert_eq!(channel.read_message(), None);
        channel
            .buffer_recv(message(Some(u32::MAX), 1, b"max"))
            .unwrap();

        assert_eq!(channel.read_message().unwrap().2, Some(MessageId(u32::MAX)));
        assert_eq!(channel.read_message().unwrap().2, Some(MessageId(0)));
    }

    #[test]
    fn unordered_unreliable_modes_do_not_expose_message_ids() {
        for mode in [
            ChannelMode::UnorderedUnreliable,
            ChannelMode::UnorderedUnreliableWithAcks,
        ] {
            let mut channel = channel(mode);
            channel
                .buffer_recv(message(Some(42), 3, b"payload"))
                .unwrap();
            assert_eq!(channel.read_message().unwrap().2, None);
        }
    }

    #[test]
    fn sequenced_unreliable_rejects_messages_older_than_the_latest_seen() {
        let mut channel = channel(ChannelMode::SequencedUnreliable);
        channel.buffer_recv(message(Some(2), 2, b"newest")).unwrap();
        channel.buffer_recv(message(Some(1), 1, b"stale")).unwrap();
        let received = channel.read_message().unwrap();
        assert_eq!(received.1, Bytes::from_static(b"newest"));
        assert_eq!(received.2, Some(MessageId(2)));
        assert_eq!(channel.read_message(), None);
    }

    #[test]
    fn sequenced_unreliable_replaces_buffered_messages_and_rejects_duplicates() {
        let mut channel = channel(ChannelMode::SequencedUnreliable);
        channel.buffer_recv(message(Some(0), 0, b"zero")).unwrap();
        channel.buffer_recv(message(Some(1), 1, b"one")).unwrap();
        channel.buffer_recv(message(Some(2), 2, b"two")).unwrap();
        channel
            .buffer_recv(message(Some(2), 3, b"duplicate"))
            .unwrap();

        let received = channel.read_message().unwrap();
        assert_eq!(received.1, Bytes::from_static(b"two"));
        assert_eq!(received.2, Some(MessageId(2)));
        assert_eq!(channel.read_message(), None);

        channel
            .buffer_recv(message(Some(2), 4, b"late duplicate"))
            .unwrap();
        assert_eq!(channel.read_message(), None);
    }

    #[test]
    fn unordered_reliable_advances_across_messages_received_out_of_order() {
        let mut channel = channel(ChannelMode::UnorderedReliable(ReliableSettings::default()));
        channel.buffer_recv(message(Some(1), 1, b"one")).unwrap();
        assert_eq!(channel.read_message().unwrap().2, Some(MessageId(1)));
        channel.buffer_recv(message(Some(0), 0, b"zero")).unwrap();
        assert_eq!(channel.read_message().unwrap().2, Some(MessageId(0)));
        channel
            .buffer_recv(message(Some(1), 2, b"duplicate"))
            .unwrap();
        assert_eq!(channel.read_message(), None);
    }

    #[test]
    fn unordered_reliable_advances_and_deduplicates_across_message_id_rollover() {
        let mut channel = channel(ChannelMode::UnorderedReliable(ReliableSettings::default()));
        let RecvState::ReliableUnordered { pending, .. } = &mut channel.state else {
            unreachable!()
        };
        *pending = MessageId(u32::MAX);

        channel.buffer_recv(message(Some(0), 0, b"zero")).unwrap();
        channel
            .buffer_recv(message(Some(u32::MAX), 1, b"max"))
            .unwrap();

        assert_eq!(channel.read_message().unwrap().2, Some(MessageId(0)));
        assert_eq!(channel.read_message().unwrap().2, Some(MessageId(u32::MAX)));
        let RecvState::ReliableUnordered { pending, .. } = &channel.state else {
            unreachable!()
        };
        assert_eq!(*pending, MessageId(1));

        channel
            .buffer_recv(message(Some(u32::MAX), 2, b"duplicate max"))
            .unwrap();
        channel
            .buffer_recv(message(Some(0), 3, b"duplicate zero"))
            .unwrap();
        assert_eq!(channel.read_message(), None);
    }

    #[test]
    fn sequenced_reliable_exposes_only_the_newest_buffered_message() {
        let mut channel = channel(ChannelMode::SequencedReliable(ReliableSettings::default()));
        channel.buffer_recv(message(Some(0), 0, b"zero")).unwrap();
        channel.buffer_recv(message(Some(1), 1, b"one")).unwrap();
        let received = channel.read_message().unwrap();
        assert_eq!(received.1, Bytes::from_static(b"one"));
        assert_eq!(received.2, Some(MessageId(1)));
        assert_eq!(channel.read_message(), None);
    }

    #[test]
    fn sequenced_reliable_does_not_redeliver_after_drain() {
        let mut channel = channel(ChannelMode::SequencedReliable(ReliableSettings::default()));
        channel.buffer_recv(message(Some(1), 1, b"one")).unwrap();
        assert_eq!(channel.read_message().unwrap().2, Some(MessageId(1)));

        channel
            .buffer_recv(message(Some(1), 2, b"duplicate"))
            .unwrap();
        assert_eq!(channel.read_message(), None);
    }
}
