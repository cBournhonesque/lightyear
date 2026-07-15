use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use bevy_time::{Real, Time, Timer};
use bytes::Bytes;
use core::time::Duration;
use lightyear_link::LinkStats;
use tracing::trace;

use crate::channel::builder::ReliableSettings;
use crate::channel::registry::{ChannelId, ChannelKind};
use crate::channel::senders::fragment_sender::FragmentSender;
use crate::packet::compression::{CompressionConfig, CompressionScratch};
use crate::packet::message::{
    FragmentData, MessageAck, MessageId, SendCandidate, SendMessage, SendMessageKey, SingleData,
};

#[derive(Debug)]
struct FragmentAck {
    data: FragmentData,
    acked: bool,
    last_sent: Option<Duration>,
}

#[derive(Debug)]
enum UnackedMessage {
    Single {
        bytes: Bytes,
        last_sent: Option<Duration>,
    },
    Fragmented(Vec<FragmentAck>),
}

#[derive(Debug)]
struct PendingReliableMessage {
    message: UnackedMessage,
    base_priority: f32,
    accumulated_priority: f32,
}

/// State which differs specifically for reliable send lanes.
#[derive(Debug)]
pub(super) struct ReliableSendState {
    settings: ReliableSettings,
    pending: BTreeMap<MessageId, PendingReliableMessage>,
    next_message_id: MessageId,
    current_rtt: Duration,
    current_time: Duration,
    priority_multiplier: f32,
}

impl ReliableSendState {
    pub(super) fn new(settings: ReliableSettings) -> Self {
        Self {
            settings,
            pending: BTreeMap::new(),
            next_message_id: MessageId::default(),
            current_rtt: Duration::default(),
            current_time: Duration::default(),
            priority_multiplier: 1.0,
        }
    }

    pub(super) fn settings(&self) -> ReliableSettings {
        self.settings
    }

    pub(super) fn update(
        &mut self,
        real_time: &Time<Real>,
        link_stats: &LinkStats,
        timer: Option<&Timer>,
    ) {
        self.current_time = real_time.elapsed();
        self.current_rtt = link_stats.rtt;
        if let Some(timer) = timer {
            self.priority_multiplier =
                timer.duration().as_nanos() as f32 / real_time.delta().as_nanos() as f32;
            trace!(
                ?timer,
                priority_multiplier = self.priority_multiplier,
                "updated reliable send-lane priority multiplier"
            );
        }
    }

    pub(super) fn buffer_send(
        &mut self,
        fragmenter: &mut FragmentSender,
        message: Bytes,
        priority: f32,
        compression: CompressionConfig,
        compression_scratch: &mut CompressionScratch,
    ) -> MessageId {
        let message_id = self.next_message_id;
        let message = if message.len() > fragmenter.fragment_size {
            UnackedMessage::Fragmented(
                fragmenter
                    .build_fragments_for_message(
                        message_id,
                        message,
                        compression,
                        compression_scratch,
                    )
                    .into_iter()
                    .map(|data| FragmentAck {
                        data,
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
        self.pending.insert(
            message_id,
            PendingReliableMessage {
                message,
                base_priority: priority,
                accumulated_priority: 0.0,
            },
        );
        self.next_message_id += 1;
        message_id
    }

    pub(super) fn collect_candidates(
        &mut self,
        channel_kind: ChannelKind,
        channel_id: ChannelId,
        output: &mut Vec<SendCandidate>,
    ) {
        let resend_delay = self.settings.resend_delay(self.current_rtt);
        let current_time = self.current_time;
        let should_send = |last_sent: &Option<Duration>| match last_sent {
            None => true,
            Some(last_sent) => {
                resend_delay != Duration::default()
                    && current_time.saturating_sub(*last_sent) > resend_delay
            }
        };

        for (message_id, pending) in &mut self.pending {
            pending.accumulated_priority += pending.base_priority * self.priority_multiplier;
            match &mut pending.message {
                UnackedMessage::Single { bytes, last_sent } if should_send(last_sent) => {
                    output.push(SendCandidate::new(
                        channel_kind,
                        channel_id,
                        SendMessageKey::ReliableSingle(*message_id),
                        SendMessage {
                            data: SingleData::new(Some(*message_id), bytes.clone()).into(),
                            priority: pending.accumulated_priority,
                        },
                    ));
                }
                UnackedMessage::Single { .. } => {}
                UnackedMessage::Fragmented(fragments) => {
                    output.extend(
                        fragments
                            .iter_mut()
                            .filter(|fragment| !fragment.acked && should_send(&fragment.last_sent))
                            .map(|fragment| {
                                SendCandidate::new(
                                    channel_kind,
                                    channel_id,
                                    SendMessageKey::ReliableFragment(
                                        *message_id,
                                        fragment.data.fragment_id,
                                    ),
                                    SendMessage {
                                        data: fragment.data.clone().into(),
                                        priority: pending.accumulated_priority,
                                    },
                                )
                            }),
                    );
                }
            }
        }
    }

    pub(super) fn commit_send(&mut self, key: SendMessageKey, sent_at: Duration) {
        match key {
            SendMessageKey::ReliableSingle(message_id) => {
                let Some(pending) = self.pending.get_mut(&message_id) else {
                    debug_assert!(false, "missing reliable message during send commit");
                    return;
                };
                let UnackedMessage::Single { last_sent, .. } = &mut pending.message else {
                    debug_assert!(false, "reliable send key did not match message shape");
                    return;
                };
                *last_sent = Some(sent_at);
            }
            SendMessageKey::ReliableFragment(message_id, fragment_id) => {
                let Some(pending) = self.pending.get_mut(&message_id) else {
                    debug_assert!(false, "missing reliable message during fragment commit");
                    return;
                };
                let UnackedMessage::Fragmented(fragments) = &mut pending.message else {
                    debug_assert!(false, "reliable fragment key did not match message shape");
                    return;
                };
                let Some(fragment) = fragments.get_mut(fragment_id.0 as usize) else {
                    debug_assert!(false, "missing reliable fragment during send commit");
                    return;
                };
                fragment.last_sent = Some(sent_at);
            }
            _ => debug_assert!(false, "unreliable key committed to reliable send lane"),
        }
    }

    pub(super) fn receive_ack(&mut self, ack: &MessageAck) {
        let Some(pending) = self.pending.get_mut(&ack.message_id) else {
            return;
        };
        match &mut pending.message {
            UnackedMessage::Single { .. } => {
                assert!(
                    ack.fragment_id.is_none(),
                    "received a fragment ack for a single reliable message"
                );
                self.pending.remove(&ack.message_id);
            }
            UnackedMessage::Fragmented(fragments) => {
                let fragment_id = ack
                    .fragment_id
                    .expect("received a single-message ack for a fragmented reliable message");
                let fragment = &mut fragments[fragment_id.0 as usize];
                if !fragment.acked {
                    fragment.acked = true;
                    if fragments.iter().all(|fragment| fragment.acked) {
                        self.pending.remove(&ack.message_id);
                    }
                }
            }
        }
    }
}
