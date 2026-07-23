//! Channel send state and delivery policies.

use alloc::collections::VecDeque;
use alloc::vec::Vec;
use bevy_time::{Real, Time, Timer, TimerMode};
use bevy_utils::prelude::DebugName;
use bytes::Bytes;
use core::time::Duration;
use lightyear_link::LinkStats;

use crate::channel::builder::{ChannelMode, ChannelSettings};
use crate::channel::registry::{ChannelId, ChannelKind};
use crate::packet::compression::{CompressionConfig, CompressionScratch};
use crate::packet::message::{
    MessageAck, MessageData, MessageId, SendCandidate, SendMessage, SendMessageKey, SingleData,
};

use crate::channel::fragments::{FragmentAckReceiver, FragmentSender};
use crate::channel::send_reliable::ReliableSendState;
const DISCARD_AFTER: Duration = Duration::from_millis(3000);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SendFlushOutcome {
    Complete,
    BandwidthLimited,
    StagingFailed,
}

/// The sending half of one registered transport channel.
///
/// Identity, fragmentation, cadence, and per-frame events are common to every mode. The private
/// state enum contains only the queueing and acknowledgement state which actually differs.
#[derive(Debug)]
pub struct ChannelSend {
    channel_kind: ChannelKind,
    channel_id: ChannelId,
    name: DebugName,
    mode: ChannelMode,
    fragmenter: FragmentSender,
    timer: Option<Timer>,
    state: SendState,
    pub(crate) message_acks: Vec<MessageId>,
    pub(crate) message_nacks: Vec<MessageId>,
    pub(crate) messages_sent: Vec<MessageId>,
}

#[derive(Debug)]
enum SendState {
    Unreliable(UnreliableSendState),
    Reliable(ReliableSendState),
}

#[derive(Debug)]
struct UnreliableSendState {
    delivery: UnreliableDelivery,
    singles: VecDeque<PendingSendMessage>,
    fragments: VecDeque<PendingSendMessage>,
    next_message_id: MessageId,
    fragment_acks: Option<FragmentAckReceiver>,
    retry_unsent_messages: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum UnreliableDelivery {
    Unordered,
    Sequenced,
    UnorderedWithAcks,
}

/// Channel-owned state for an unreliable message during speculative packet staging.
#[derive(Debug)]
struct PendingSendMessage {
    message: SendMessage,
    committed: bool,
    /// Once one fragment is admitted, retain the rest even on a discard-unsent channel.
    fragment_started: bool,
}

impl PendingSendMessage {
    fn new(message: SendMessage) -> Self {
        Self {
            message,
            committed: false,
            fragment_started: false,
        }
    }

    fn fragment_message_id(&self) -> Option<MessageId> {
        let MessageData::Fragment(fragment) = &self.message.data else {
            return None;
        };
        Some(fragment.message_id)
    }
}

impl ChannelSend {
    pub(crate) fn new(
        channel_kind: ChannelKind,
        channel_id: ChannelId,
        name: DebugName,
        settings: &ChannelSettings,
        fragment_size: usize,
    ) -> Self {
        let state = match settings.mode {
            ChannelMode::UnorderedUnreliableWithAcks => {
                SendState::Unreliable(UnreliableSendState::new(
                    UnreliableDelivery::UnorderedWithAcks,
                    settings.retry_unsent_messages,
                ))
            }
            ChannelMode::UnorderedUnreliable => SendState::Unreliable(UnreliableSendState::new(
                UnreliableDelivery::Unordered,
                settings.retry_unsent_messages,
            )),
            ChannelMode::SequencedUnreliable => SendState::Unreliable(UnreliableSendState::new(
                UnreliableDelivery::Sequenced,
                settings.retry_unsent_messages,
            )),
            ChannelMode::UnorderedReliable(settings)
            | ChannelMode::SequencedReliable(settings)
            | ChannelMode::OrderedReliable(settings) => {
                SendState::Reliable(ReliableSendState::new(settings))
            }
        };
        let timer = (settings.send_frequency != Duration::default())
            .then(|| Timer::new(settings.send_frequency, TimerMode::Repeating));
        let mut fragmenter = FragmentSender::new();
        fragmenter.set_fragment_size(fragment_size);
        Self {
            channel_kind,
            channel_id,
            name,
            mode: settings.mode,
            fragmenter,
            timer,
            state,
            message_acks: Vec::new(),
            message_nacks: Vec::new(),
            messages_sent: Vec::new(),
        }
    }

    /// The type-level channel key associated with this channel.
    pub fn channel_kind(&self) -> ChannelKind {
        self.channel_kind
    }

    /// The compact channel identifier encoded on the wire.
    pub fn channel_id(&self) -> ChannelId {
        self.channel_id
    }

    /// The registered channel name used by diagnostics.
    pub fn name(&self) -> &DebugName {
        &self.name
    }

    /// The ordering and reliability mode implemented by this channel.
    pub fn mode(&self) -> ChannelMode {
        self.mode
    }

    /// Message IDs acknowledged during the current frame.
    pub fn message_acks(&self) -> &[MessageId] {
        &self.message_acks
    }

    /// Message IDs whose packets were declared lost during the current frame.
    pub fn message_nacks(&self) -> &[MessageId] {
        &self.message_nacks
    }

    /// Message IDs admitted to the link during the current frame.
    pub fn messages_sent(&self) -> &[MessageId] {
        &self.messages_sent
    }

    pub(crate) fn watches_acks(&self) -> bool {
        matches!(
            &self.state,
            SendState::Reliable(_)
                | SendState::Unreliable(UnreliableSendState {
                    delivery: UnreliableDelivery::UnorderedWithAcks,
                    ..
                })
        )
    }

    pub(crate) fn set_fragment_size(&mut self, fragment_size: usize) {
        self.fragmenter.set_fragment_size(fragment_size);
    }

    pub(crate) fn update(&mut self, real_time: &Time<Real>, link_stats: &LinkStats) {
        if let Some(timer) = &mut self.timer {
            timer.tick(real_time.delta());
        }
        match &mut self.state {
            SendState::Unreliable(state) => state.update(real_time),
            SendState::Reliable(state) => {
                state.update(real_time, link_stats, self.timer.as_ref());
            }
        }
    }

    pub(crate) fn clear_frame_events(&mut self) {
        self.message_acks.clear();
        self.message_nacks.clear();
        self.messages_sent.clear();
    }

    pub(crate) fn buffer_send_with_scratch(
        &mut self,
        message: Bytes,
        priority: f32,
        compression: CompressionConfig,
        compression_scratch: &mut CompressionScratch,
    ) -> Option<MessageId> {
        match &mut self.state {
            SendState::Unreliable(state) => state.buffer_send(
                &mut self.fragmenter,
                message,
                priority,
                compression,
                compression_scratch,
            ),
            SendState::Reliable(state) => Some(state.buffer_send(
                &mut self.fragmenter,
                message,
                priority,
                compression,
                compression_scratch,
            )),
        }
    }

    #[cfg(test)]
    fn buffer_send(
        &mut self,
        message: Bytes,
        priority: f32,
        compression: CompressionConfig,
    ) -> Option<MessageId> {
        self.buffer_send_with_scratch(
            message,
            priority,
            compression,
            &mut CompressionScratch::default(),
        )
    }

    pub(crate) fn collect_send_candidates(&mut self, output: &mut Vec<SendCandidate>) {
        if !is_ready_to_send(self.timer.as_ref()) {
            return;
        }
        match &mut self.state {
            SendState::Unreliable(state) => {
                state.collect_candidates(self.channel_kind, self.channel_id, output);
            }
            SendState::Reliable(state) => {
                state.collect_candidates(self.channel_kind, self.channel_id, output);
            }
        }
    }

    pub(crate) fn commit_send(&mut self, key: SendMessageKey, sent_at: Duration) {
        match &mut self.state {
            SendState::Unreliable(state) => state.commit_send(key),
            SendState::Reliable(state) => state.commit_send(key, sent_at),
        }
    }

    pub(crate) fn finish_send(&mut self, outcome: SendFlushOutcome) {
        if let SendState::Unreliable(state) = &mut self.state {
            state.finish_send(outcome, is_ready_to_send(self.timer.as_ref()));
        }
    }

    /// Applies an acknowledgement and returns whether it completed the logical message.
    pub(crate) fn receive_ack(&mut self, ack: &MessageAck) -> bool {
        match &mut self.state {
            SendState::Unreliable(state) => state.receive_ack(ack),
            SendState::Reliable(state) => state.receive_ack(ack),
        }
    }

    /// Discards acknowledgement completion state which can no longer complete after packet loss.
    pub(crate) fn receive_nack(&mut self, nack: &MessageAck) {
        if let SendState::Unreliable(state) = &mut self.state {
            state.receive_nack(nack);
        }
    }
}

impl UnreliableSendState {
    fn new(delivery: UnreliableDelivery, retry_unsent_messages: bool) -> Self {
        Self {
            delivery,
            singles: VecDeque::new(),
            fragments: VecDeque::new(),
            next_message_id: MessageId::default(),
            fragment_acks: (delivery == UnreliableDelivery::UnorderedWithAcks)
                .then(FragmentAckReceiver::new),
            retry_unsent_messages,
        }
    }

    fn update(&mut self, real_time: &Time<Real>) {
        if let Some(fragment_acks) = &mut self.fragment_acks {
            fragment_acks.cleanup(real_time.elapsed().saturating_sub(DISCARD_AFTER));
        }
    }

    fn buffer_send(
        &mut self,
        fragmenter: &mut FragmentSender,
        message: Bytes,
        priority: f32,
        compression: CompressionConfig,
        compression_scratch: &mut CompressionScratch,
    ) -> Option<MessageId> {
        let message_id = self.next_message_id;
        let is_fragmented = message.len() > fragmenter.fragment_size;
        let has_id = is_fragmented || self.delivery != UnreliableDelivery::Unordered;

        if is_fragmented {
            let fragments = fragmenter.build_fragments_for_message(
                message_id,
                message,
                compression,
                compression_scratch,
            );
            if let Some(fragment_acks) = &mut self.fragment_acks {
                fragment_acks.add_new_fragment_to_wait_for(message_id, fragments.len());
            }
            self.fragments.extend(fragments.into_iter().map(|fragment| {
                PendingSendMessage::new(SendMessage {
                    data: fragment.into(),
                    priority,
                })
            }));
        } else {
            self.singles.push_back(PendingSendMessage::new(SendMessage {
                data: SingleData::new(has_id.then_some(message_id), message).into(),
                priority,
            }));
        }

        if has_id {
            self.next_message_id += 1;
            Some(message_id)
        } else {
            None
        }
    }

    fn collect_candidates(
        &self,
        channel_kind: ChannelKind,
        channel_id: ChannelId,
        output: &mut Vec<SendCandidate>,
    ) {
        output.extend(
            self.singles
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
            self.fragments
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

    fn commit_send(&mut self, key: SendMessageKey) {
        let committed = commit_unreliable_candidate(&mut self.singles, &mut self.fragments, key);
        debug_assert!(committed, "invalid unreliable send candidate");
    }

    fn finish_send(&mut self, outcome: SendFlushOutcome, was_ready: bool) {
        let discard_unsent = was_ready
            && !self.retry_unsent_messages
            && outcome == SendFlushOutcome::BandwidthLimited;
        if discard_unsent {
            if let Some(fragment_acks) = &mut self.fragment_acks {
                for pending in self
                    .fragments
                    .iter()
                    .filter(|pending| !pending.committed && !pending.fragment_started)
                {
                    if let MessageData::Fragment(fragment) = &pending.message.data
                        && fragment.fragment_id.0 == 0
                    {
                        fragment_acks.discard_message(fragment.message_id);
                    }
                }
            }
            self.singles.clear();
            self.fragments
                .retain(|pending| !pending.committed && pending.fragment_started);
        } else {
            self.singles.retain(|pending| !pending.committed);
            self.fragments.retain(|pending| !pending.committed);
        }
    }

    fn receive_ack(&mut self, ack: &MessageAck) -> bool {
        let Some(fragment_index) = ack.fragment_id else {
            return self.delivery == UnreliableDelivery::UnorderedWithAcks;
        };
        self.fragment_acks.as_mut().is_some_and(|fragment_acks| {
            fragment_acks.receive_fragment_ack(ack.message_id, fragment_index, None)
        })
    }

    fn receive_nack(&mut self, nack: &MessageAck) {
        if nack.fragment_id.is_some()
            && let Some(fragment_acks) = &mut self.fragment_acks
        {
            fragment_acks.discard_message(nack.message_id);
        }
    }
}

fn is_ready_to_send(timer: Option<&Timer>) -> bool {
    timer.is_none_or(Timer::is_finished)
}

fn commit_unreliable_candidate(
    singles: &mut VecDeque<PendingSendMessage>,
    fragments: &mut VecDeque<PendingSendMessage>,
    key: SendMessageKey,
) -> bool {
    match key {
        SendMessageKey::UnreliableSingle(index) => {
            let Some(pending) = singles.get_mut(index) else {
                return false;
            };
            pending.committed = true;
        }
        SendMessageKey::UnreliableFragment(index) => {
            let Some(pending) = fragments.get_mut(index) else {
                return false;
            };
            let message_id = pending
                .fragment_message_id()
                .expect("fragment queue must contain fragment data");
            let mark_started = !pending.fragment_started;
            pending.committed = true;
            if mark_started {
                for pending in fragments {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channel::builder::ReliableSettings;
    use crate::packet::message::FragmentIndex;
    use crate::packet::packet::FRAGMENT_SIZE;
    use alloc::vec;

    struct TestChannel;

    fn channel(mode: ChannelMode, retry_unsent_messages: bool) -> ChannelSend {
        ChannelSend::new(
            ChannelKind::of::<TestChannel>(),
            0,
            DebugName::type_name::<TestChannel>(),
            &ChannelSettings {
                mode,
                retry_unsent_messages,
                ..ChannelSettings::default()
            },
            FRAGMENT_SIZE,
        )
    }

    fn fragment_ack(fragment_id: u64) -> MessageAck {
        MessageAck {
            message_id: MessageId(0),
            fragment_id: Some(FragmentIndex(fragment_id)),
        }
    }

    #[test]
    fn every_channel_mode_constructs_one_channel_send() {
        for (mode, watches_acks) in [
            (ChannelMode::UnorderedUnreliable, false),
            (ChannelMode::UnorderedUnreliableWithAcks, true),
            (ChannelMode::SequencedUnreliable, false),
            (
                ChannelMode::UnorderedReliable(ReliableSettings::default()),
                true,
            ),
            (
                ChannelMode::OrderedReliable(ReliableSettings::default()),
                true,
            ),
            (
                ChannelMode::SequencedReliable(ReliableSettings::default()),
                true,
            ),
        ] {
            let channel = channel(mode, true);
            assert_eq!(channel.mode(), mode);
            assert_eq!(channel.watches_acks(), watches_acks);
        }
    }

    #[test]
    fn retry_unsent_messages_defaults_to_true() {
        assert!(ChannelSettings::default().retry_unsent_messages);
    }

    #[test]
    fn unreliable_candidates_remain_pending_until_commit() {
        let mut channel = channel(ChannelMode::UnorderedUnreliable, true);
        channel.buffer_send(
            Bytes::from_static(b"pending"),
            1.0,
            CompressionConfig::DISABLED,
        );
        let mut candidates = Vec::new();
        channel.collect_send_candidates(&mut candidates);
        assert_eq!(candidates.len(), 1);
        channel.finish_send(SendFlushOutcome::BandwidthLimited);
        candidates.clear();
        channel.collect_send_candidates(&mut candidates);
        assert_eq!(candidates.len(), 1);
        channel.commit_send(candidates[0].key, Duration::default());
        channel.finish_send(SendFlushOutcome::Complete);
        candidates.clear();
        channel.collect_send_candidates(&mut candidates);
        assert!(candidates.is_empty());
    }

    #[test]
    fn reliable_channel_retains_locally_unadmitted_messages() {
        let mut channel = channel(
            ChannelMode::UnorderedReliable(ReliableSettings::default()),
            false,
        );
        channel.buffer_send(
            Bytes::from_static(b"reliable"),
            1.0,
            CompressionConfig::DISABLED,
        );
        let mut candidates = Vec::new();
        channel.collect_send_candidates(&mut candidates);
        assert_eq!(candidates.len(), 1);
        channel.finish_send(SendFlushOutcome::BandwidthLimited);
        candidates.clear();
        channel.collect_send_candidates(&mut candidates);
        assert_eq!(candidates.len(), 1);
    }

    #[test]
    fn unreliable_channel_can_discard_locally_unadmitted_messages() {
        let mut channel = channel(ChannelMode::UnorderedUnreliable, false);
        channel.buffer_send(
            Bytes::from_static(b"stale"),
            1.0,
            CompressionConfig::DISABLED,
        );
        let mut candidates = Vec::new();
        channel.collect_send_candidates(&mut candidates);
        assert_eq!(candidates.len(), 1);
        channel.finish_send(SendFlushOutcome::BandwidthLimited);
        candidates.clear();
        channel.collect_send_candidates(&mut candidates);
        assert!(candidates.is_empty());
    }

    #[test]
    fn discard_policy_only_applies_after_bandwidth_limiting() {
        let mut channel = channel(ChannelMode::UnorderedUnreliable, false);
        channel.buffer_send(
            Bytes::from_static(b"stale"),
            1.0,
            CompressionConfig::DISABLED,
        );

        let mut candidates = Vec::new();
        channel.collect_send_candidates(&mut candidates);
        channel.finish_send(SendFlushOutcome::StagingFailed);
        candidates.clear();
        channel.collect_send_candidates(&mut candidates);
        assert_eq!(candidates.len(), 1);

        channel.finish_send(SendFlushOutcome::BandwidthLimited);
        candidates.clear();
        channel.collect_send_candidates(&mut candidates);
        assert!(candidates.is_empty());
    }

    #[test]
    fn discard_policy_finishes_started_fragmented_messages() {
        let message_len = FRAGMENT_SIZE * 2 + 1;

        let mut wholly_unsent = channel(ChannelMode::UnorderedUnreliable, false);
        wholly_unsent.buffer_send(
            Bytes::from(vec![0; message_len]),
            1.0,
            CompressionConfig::DISABLED,
        );
        let mut candidates = Vec::new();
        wholly_unsent.collect_send_candidates(&mut candidates);
        assert!(candidates.len() > 1);
        wholly_unsent.finish_send(SendFlushOutcome::BandwidthLimited);
        candidates.clear();
        wholly_unsent.collect_send_candidates(&mut candidates);
        assert!(candidates.is_empty());

        let mut partially_sent = channel(ChannelMode::UnorderedUnreliable, false);
        partially_sent.buffer_send(
            Bytes::from(vec![0; message_len]),
            1.0,
            CompressionConfig::DISABLED,
        );
        partially_sent.collect_send_candidates(&mut candidates);
        let fragment_count = candidates.len();
        partially_sent.commit_send(candidates[0].key, Duration::default());
        partially_sent.finish_send(SendFlushOutcome::BandwidthLimited);
        candidates.clear();
        partially_sent.collect_send_candidates(&mut candidates);
        assert_eq!(candidates.len(), fragment_count - 1);
    }

    #[test]
    fn bandwidth_limit_does_not_discard_before_send_frequency() {
        let mut channel = ChannelSend::new(
            ChannelKind::of::<TestChannel>(),
            0,
            DebugName::type_name::<TestChannel>(),
            &ChannelSettings {
                mode: ChannelMode::UnorderedUnreliable,
                send_frequency: Duration::from_secs(1),
                retry_unsent_messages: false,
                ..ChannelSettings::default()
            },
            FRAGMENT_SIZE,
        );
        channel.buffer_send(
            Bytes::from_static(b"not-yet-eligible"),
            1.0,
            CompressionConfig::DISABLED,
        );

        let mut candidates = Vec::new();
        channel.collect_send_candidates(&mut candidates);
        assert!(candidates.is_empty());
        channel.finish_send(SendFlushOutcome::BandwidthLimited);

        let mut real = Time::<Real>::default();
        real.advance_by(Duration::from_secs(1));
        channel.update(&real, &LinkStats::default());
        channel.collect_send_candidates(&mut candidates);
        assert_eq!(candidates.len(), 1);
    }

    #[test]
    fn discarding_wholly_unsent_fragments_clears_ack_tracking() {
        let mut channel = channel(ChannelMode::UnorderedUnreliableWithAcks, false);
        channel.buffer_send(
            Bytes::from(vec![0; FRAGMENT_SIZE * 2 + 1]),
            1.0,
            CompressionConfig::DISABLED,
        );
        let mut candidates = Vec::new();
        channel.collect_send_candidates(&mut candidates);
        assert!(candidates.len() > 1);

        channel.finish_send(SendFlushOutcome::BandwidthLimited);
        let SendState::Unreliable(state) = &channel.state else {
            panic!("expected an unreliable channel");
        };
        assert_eq!(state.fragment_acks, Some(FragmentAckReceiver::new()));
    }

    #[test]
    fn unreliable_fragment_ack_completion_is_channel_owned_and_idempotent() {
        let mut channel_a = channel(ChannelMode::UnorderedUnreliableWithAcks, true);
        let mut channel_b = channel(ChannelMode::UnorderedUnreliableWithAcks, true);
        for channel in [&mut channel_a, &mut channel_b] {
            assert_eq!(
                channel.buffer_send(
                    Bytes::from(vec![0; FRAGMENT_SIZE + 1]),
                    1.0,
                    CompressionConfig::DISABLED,
                ),
                Some(MessageId(0))
            );
        }

        assert!(!channel_a.receive_ack(&fragment_ack(0)));
        assert!(!channel_a.receive_ack(&fragment_ack(0)));
        assert!(!channel_b.receive_ack(&fragment_ack(0)));
        assert!(channel_a.receive_ack(&fragment_ack(1)));
        assert!(!channel_a.receive_ack(&fragment_ack(1)));
        assert!(channel_b.receive_ack(&fragment_ack(1)));
    }

    #[test]
    fn unreliable_fragment_nack_discards_completion_state() {
        let mut channel = channel(ChannelMode::UnorderedUnreliableWithAcks, true);
        channel.buffer_send(
            Bytes::from(vec![0; FRAGMENT_SIZE + 1]),
            1.0,
            CompressionConfig::DISABLED,
        );

        channel.receive_nack(&fragment_ack(0));
        assert!(!channel.receive_ack(&fragment_ack(1)));
        assert!(!channel.receive_ack(&fragment_ack(0)));
    }

    #[test]
    fn reliable_fragment_ack_reports_completion_once_and_clears_retries() {
        let mut channel = channel(
            ChannelMode::UnorderedReliable(ReliableSettings::default()),
            true,
        );
        assert_eq!(
            channel.buffer_send(
                Bytes::from(vec![0; FRAGMENT_SIZE + 1]),
                1.0,
                CompressionConfig::DISABLED,
            ),
            Some(MessageId(0))
        );

        assert!(!channel.receive_ack(&fragment_ack(0)));
        assert!(!channel.receive_ack(&fragment_ack(0)));
        assert!(channel.receive_ack(&fragment_ack(1)));
        assert!(!channel.receive_ack(&fragment_ack(1)));

        let mut candidates = Vec::new();
        channel.collect_send_candidates(&mut candidates);
        assert!(candidates.is_empty());
    }

    #[test]
    fn message_id_policy_is_derived_from_delivery_mode() {
        let mut unordered = channel(ChannelMode::UnorderedUnreliable, true);
        assert_eq!(
            unordered.buffer_send(
                Bytes::from_static(b"single"),
                1.0,
                CompressionConfig::DISABLED,
            ),
            None
        );
        assert_eq!(
            unordered.buffer_send(
                Bytes::from(vec![0; FRAGMENT_SIZE + 1]),
                1.0,
                CompressionConfig::DISABLED,
            ),
            Some(MessageId(0))
        );

        let mut sequenced = channel(ChannelMode::SequencedUnreliable, true);
        assert_eq!(
            sequenced.buffer_send(
                Bytes::from_static(b"single"),
                1.0,
                CompressionConfig::DISABLED,
            ),
            Some(MessageId(0))
        );
    }

    #[test]
    fn every_reliable_mode_retries_after_the_rtt_delay_and_stops_after_ack() {
        let reliable = ReliableSettings {
            rtt_resend_factor: 1.0,
            rtt_resend_min_delay: Duration::from_millis(100),
        };
        for mode in [
            ChannelMode::UnorderedReliable(reliable),
            ChannelMode::SequencedReliable(reliable),
            ChannelMode::OrderedReliable(reliable),
        ] {
            let mut channel = channel(mode, true);
            channel.buffer_send(
                Bytes::from_static(b"reliable"),
                1.0,
                CompressionConfig::DISABLED,
            );
            let mut candidates = Vec::new();
            channel.collect_send_candidates(&mut candidates);
            assert_eq!(candidates.len(), 1);
            channel.commit_send(candidates[0].key, Duration::ZERO);

            let mut time = Time::<Real>::default();
            time.advance_by(Duration::from_millis(100));
            channel.update(&time, &LinkStats::default());
            candidates.clear();
            channel.collect_send_candidates(&mut candidates);
            assert!(candidates.is_empty());

            time.advance_by(Duration::from_millis(1));
            channel.update(&time, &LinkStats::default());
            channel.collect_send_candidates(&mut candidates);
            assert_eq!(candidates.len(), 1);
            assert!(channel.receive_ack(&MessageAck {
                message_id: MessageId(0),
                fragment_id: None,
            }));
            candidates.clear();
            channel.collect_send_candidates(&mut candidates);
            assert!(candidates.is_empty());
        }
    }
}
