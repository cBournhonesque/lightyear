//! Channel receive lanes and delivery-policy state.

use alloc::collections::{BTreeMap, VecDeque};
use bevy_platform::collections::HashSet;
use bytes::Bytes;
use core::time::Duration;
use lightyear_core::tick::Tick;

use crate::channel::builder::{ChannelMode, ChannelSettings};
use crate::channel::registry::ChannelKind;
use crate::packet::message::{MessageData, MessageId, ReceiveMessage};

use self::error::{ChannelReceiveError, Result};
use self::fragment_receiver::FragmentReceiver;

pub mod error;
pub(crate) mod fragment_receiver;

const DISCARD_AFTER: Duration = Duration::from_millis(3000);

type ReceivedMessage = (Tick, Bytes);

/// The receiving half of one registered transport channel.
///
/// Fragment reassembly and time bookkeeping are shared by every delivery mode. Only the policy
/// for accepting and exposing completed messages varies through the private state enum.
#[derive(Debug)]
pub struct RecvLane {
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
        ready: VecDeque<(Tick, Bytes, MessageId)>,
        most_recent: MessageId,
    },
    ReliableUnordered {
        pending: MessageId,
        ready: BTreeMap<MessageId, (Tick, Bytes, MessageId)>,
        received: HashSet<MessageId>,
    },
    ReliableOrdered {
        pending: MessageId,
        ready: BTreeMap<MessageId, ReceivedMessage>,
    },
    ReliableSequenced {
        ready: BTreeMap<MessageId, ReceivedMessage>,
        most_recent: MessageId,
    },
}

impl RecvLane {
    pub(crate) fn new(channel_kind: ChannelKind, settings: &ChannelSettings) -> Self {
        let state = match settings.mode {
            ChannelMode::UnorderedUnreliableWithAcks | ChannelMode::UnorderedUnreliable => {
                RecvState::UnreliableUnordered {
                    ready: VecDeque::new(),
                }
            }
            ChannelMode::SequencedUnreliable => RecvState::UnreliableSequenced {
                ready: VecDeque::new(),
                most_recent: MessageId::default(),
            },
            ChannelMode::UnorderedReliable(_) => RecvState::ReliableUnordered {
                pending: MessageId::default(),
                ready: BTreeMap::new(),
                received: HashSet::default(),
            },
            ChannelMode::OrderedReliable(_) => RecvState::ReliableOrdered {
                pending: MessageId::default(),
                ready: BTreeMap::new(),
            },
            ChannelMode::SequencedReliable(_) => RecvState::ReliableSequenced {
                ready: BTreeMap::new(),
                most_recent: MessageId::default(),
            },
        };
        Self {
            channel_kind,
            fragments: FragmentReceiver::new(),
            current_time: Duration::default(),
            state,
        }
    }

    /// The type-level channel key associated with this lane.
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

    /// Removes and returns the next message accepted by the lane's delivery policy.
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
                most_recent,
                ready: _,
            } => {
                let message_id = message_id.ok_or(ChannelReceiveError::MissingMessageId)?;
                if message_id < *most_recent {
                    return Ok(false);
                }
                if message_id > *most_recent {
                    *most_recent = message_id;
                }
                Ok(true)
            }
            Self::ReliableUnordered {
                pending,
                ready,
                received,
            } => {
                let message_id = message_id.ok_or(ChannelReceiveError::MissingMessageId)?;
                Ok(message_id >= *pending
                    && !ready.contains_key(&message_id)
                    && !received.contains(&message_id))
            }
            Self::ReliableOrdered { pending, ready } => {
                let message_id = message_id.ok_or(ChannelReceiveError::MissingMessageId)?;
                Ok(message_id >= *pending && !ready.contains_key(&message_id))
            }
            Self::ReliableSequenced { ready, most_recent } => {
                let message_id = message_id.ok_or(ChannelReceiveError::MissingMessageId)?;
                if message_id < *most_recent {
                    return Ok(false);
                }
                if message_id > *most_recent {
                    *most_recent = message_id;
                }
                Ok(!ready.contains_key(&message_id))
            }
        }
    }

    fn push_completed(&mut self, message_id: Option<MessageId>, tick: Tick, bytes: Bytes) {
        match self {
            Self::UnreliableUnordered { ready } => ready.push_back((tick, bytes)),
            Self::UnreliableSequenced { ready, .. } => ready.push_back((
                tick,
                bytes,
                message_id.expect("sequenced messages have ids"),
            )),
            Self::ReliableUnordered {
                ready, received, ..
            } => {
                let message_id = message_id.expect("reliable messages have ids");
                received.insert(message_id);
                ready.insert(message_id, (tick, bytes, message_id));
            }
            Self::ReliableOrdered { ready, .. } | Self::ReliableSequenced { ready, .. } => {
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
            Self::UnreliableSequenced { ready, .. } => ready
                .pop_front()
                .map(|(tick, bytes, id)| (tick, bytes, Some(id))),
            Self::ReliableUnordered {
                pending,
                ready,
                received,
            } => {
                let (message_id, (tick, bytes, returned_id)) = ready.pop_first()?;
                if *pending == message_id {
                    while received.remove(pending) {
                        *pending += 1;
                    }
                }
                Some((tick, bytes, Some(returned_id)))
            }
            Self::ReliableOrdered { pending, ready } => {
                let (tick, bytes) = ready.remove(pending)?;
                let message_id = *pending;
                *pending += 1;
                Some((tick, bytes, Some(message_id)))
            }
            Self::ReliableSequenced { ready, most_recent } => loop {
                let (message_id, (tick, bytes)) = ready.pop_first()?;
                if message_id >= *most_recent {
                    return Some((tick, bytes, Some(message_id)));
                }
            },
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

    fn lane(mode: ChannelMode) -> RecvLane {
        RecvLane::new(
            ChannelKind::of::<TestChannel>(),
            &ChannelSettings {
                mode,
                ..ChannelSettings::default()
            },
        )
    }

    #[test]
    fn every_channel_mode_constructs_one_receive_lane() {
        for mode in [
            ChannelMode::UnorderedUnreliable,
            ChannelMode::UnorderedUnreliableWithAcks,
            ChannelMode::SequencedUnreliable,
            ChannelMode::UnorderedReliable(ReliableSettings::default()),
            ChannelMode::OrderedReliable(ReliableSettings::default()),
            ChannelMode::SequencedReliable(ReliableSettings::default()),
        ] {
            assert_eq!(lane(mode).channel_kind(), ChannelKind::of::<TestChannel>());
        }
    }

    #[test]
    fn ordered_reliable_waits_for_the_missing_sequence() {
        let mut lane = lane(ChannelMode::OrderedReliable(ReliableSettings::default()));
        lane.buffer_recv(message(Some(1), 1, b"one")).unwrap();
        assert_eq!(lane.read_message(), None);
        lane.buffer_recv(message(Some(0), 0, b"zero")).unwrap();
        assert_eq!(lane.read_message().unwrap().2, Some(MessageId(0)));
        assert_eq!(lane.read_message().unwrap().2, Some(MessageId(1)));
    }

    #[test]
    fn unreliable_unordered_does_not_expose_message_ids() {
        let mut lane = lane(ChannelMode::UnorderedUnreliable);
        lane.buffer_recv(message(Some(42), 3, b"payload")).unwrap();
        assert_eq!(lane.read_message().unwrap().2, None);
    }

    #[test]
    fn sequenced_unreliable_rejects_messages_older_than_the_latest_seen() {
        let mut lane = lane(ChannelMode::SequencedUnreliable);
        lane.buffer_recv(message(Some(2), 2, b"newest")).unwrap();
        lane.buffer_recv(message(Some(1), 1, b"stale")).unwrap();
        let received = lane.read_message().unwrap();
        assert_eq!(received.1, Bytes::from_static(b"newest"));
        assert_eq!(received.2, Some(MessageId(2)));
        assert_eq!(lane.read_message(), None);
    }

    #[test]
    fn unordered_reliable_advances_across_messages_received_out_of_order() {
        let mut lane = lane(ChannelMode::UnorderedReliable(ReliableSettings::default()));
        lane.buffer_recv(message(Some(1), 1, b"one")).unwrap();
        assert_eq!(lane.read_message().unwrap().2, Some(MessageId(1)));
        lane.buffer_recv(message(Some(0), 0, b"zero")).unwrap();
        assert_eq!(lane.read_message().unwrap().2, Some(MessageId(0)));
        lane.buffer_recv(message(Some(1), 2, b"duplicate")).unwrap();
        assert_eq!(lane.read_message(), None);
    }

    #[test]
    fn sequenced_reliable_exposes_only_the_newest_buffered_message() {
        let mut lane = lane(ChannelMode::SequencedReliable(ReliableSettings::default()));
        lane.buffer_recv(message(Some(0), 0, b"zero")).unwrap();
        lane.buffer_recv(message(Some(1), 1, b"one")).unwrap();
        let received = lane.read_message().unwrap();
        assert_eq!(received.1, Bytes::from_static(b"one"));
        assert_eq!(received.2, Some(MessageId(1)));
        assert_eq!(lane.read_message(), None);
    }
}
