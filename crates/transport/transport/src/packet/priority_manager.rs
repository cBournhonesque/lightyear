use crate::channel::registry::ChannelRegistry;
use crate::packet::message::{MessageData, SendCandidate};
use alloc::vec::Vec;
use core::num::NonZeroU32;
use governor::{DefaultDirectRateLimiter, Quota};
use lightyear_serde::ToBytes;
use nonzero_ext::*;
#[cfg(feature = "trace")]
use tracing::{Level, instrument};
#[allow(unused_imports)]
use tracing::{debug, error, info, trace};

#[derive(Debug, Clone)]
pub struct PriorityConfig {
    /// Number of bytes per second that can be sent to each client
    pub bandwidth_quota: Quota,
    /// If false, there is no bandwidth cap and all messages are sent as soon as possible
    pub enabled: bool,
}

// this is mostly for testing
impl Default for PriorityConfig {
    fn default() -> Self {
        Self {
            // 56 KB/s bandwidth cap
            bandwidth_quota: Quota::per_second(nonzero!(56000u32)),
            enabled: false,
        }
    }
}

impl PriorityConfig {
    /// Enables a sustained byte rate with an equally sized burst capacity.
    ///
    /// The bandwidth quota is independent of link MTU. Use
    /// [`with_burst_size`](Self::with_burst_size) when the burst should differ from the sustained
    /// byte rate, or provide a custom [`Quota`] through [`Self::bandwidth_quota`].
    pub fn new(bytes_per_second_quota: u32) -> Self {
        let cap = bytes_per_second_quota.try_into().unwrap();
        Self {
            bandwidth_quota: Quota::per_second(cap).allow_burst(cap),
            enabled: true,
        }
    }

    /// Sets the maximum bandwidth burst independently of the sustained byte rate and link MTU.
    pub fn with_burst_size(mut self, burst_size: u32) -> Self {
        let burst_size = burst_size.try_into().unwrap();
        self.bandwidth_quota = self.bandwidth_quota.allow_burst(burst_size);
        self
    }
}

/// Reusable scheduler scratch for ordering channel-owned send candidates.
///
/// This type never owns channel queues and does not decide whether a packet was sent. Bandwidth is
/// consumed separately by [`BandwidthLimiter`] after packet staging and compression.
#[derive(Debug)]
pub struct PriorityManager {
    pub(crate) config: PriorityConfig,
    /// Reused scratch containing cheap snapshots of channel-owned pending messages.
    candidates: Vec<SendCandidate>,
}

/// Consumes the configured bandwidth quota using final packet bytes.
#[derive(Debug)]
pub(crate) struct BandwidthLimiter {
    enabled: bool,
    limiter: DefaultDirectRateLimiter,
}

impl Default for PriorityManager {
    fn default() -> Self {
        Self::new(PriorityConfig::default())
    }
}

impl PriorityManager {
    pub fn new(config: PriorityConfig) -> Self {
        Self {
            config,
            candidates: Vec::new(),
        }
    }

    pub(crate) fn candidates_mut(&mut self) -> &mut Vec<SendCandidate> {
        &mut self.candidates
    }

    pub(crate) fn candidates(&self) -> &[SendCandidate] {
        &self.candidates
    }

    pub(crate) fn clear(&mut self) {
        self.candidates.clear();
    }

    /// Apply priority and packet-packing order without taking ownership of channel queues.
    ///
    /// Fragments precede singles within each priority group so the packet builder can fill the
    /// final fragment packets with the smallest remaining singles. When priority management is
    /// disabled, every candidate belongs to one packing group.
    #[cfg_attr(feature = "trace", instrument(level = Level::INFO, skip_all))]
    pub(crate) fn prioritize(&mut self, channel_registry: &ChannelRegistry) {
        if !self.config.enabled {
            for candidate in &mut self.candidates {
                candidate.effective_priority = 0.0;
            }
            self.candidates.sort_unstable_by(Self::packing_order);
            debug!(
                num_candidates = self.candidates.len(),
                "priority disabled; applied fragment-first packing order"
            );
            return;
        }

        for candidate in &mut self.candidates {
            let channel_priority = channel_registry
                .settings_from_net_id(candidate.channel_id)
                .expect("candidate channel must be registered")
                .priority;
            candidate.effective_priority = candidate.message.priority * channel_priority;
        }

        // Every candidate has deterministic tie-breakers, so the in-place unstable sort has
        // stable observable output without allocating temporary sort storage.
        self.candidates.sort_unstable_by(|a, b| {
            b.effective_priority
                .total_cmp(&a.effective_priority)
                .then_with(|| Self::packing_order(a, b))
        });
        debug!(
            num_candidates = self.candidates.len(),
            "priority ordering complete"
        );
    }

    fn packing_order(a: &SendCandidate, b: &SendCandidate) -> core::cmp::Ordering {
        match (&a.message.data, &b.message.data) {
            (MessageData::Fragment(_), MessageData::Single(_)) => core::cmp::Ordering::Less,
            (MessageData::Single(_), MessageData::Fragment(_)) => core::cmp::Ordering::Greater,
            (MessageData::Single(a_message), MessageData::Single(b_message)) => a_message
                .bytes_len()
                .cmp(&b_message.bytes_len())
                .then_with(|| a.key.send_order().cmp(&b.key.send_order())),
            (MessageData::Fragment(a_fragment), MessageData::Fragment(b_fragment)) => {
                a_fragment.message_id.cmp(&b_fragment.message_id)
            }
        }
        .then_with(|| a.channel_id.cmp(&b.channel_id))
        .then_with(|| a.key.cmp(&b.key))
    }
}

impl BandwidthLimiter {
    pub(crate) fn new(config: PriorityConfig) -> Self {
        Self {
            enabled: config.enabled,
            limiter: DefaultDirectRateLimiter::direct(config.bandwidth_quota),
        }
    }

    /// Consume bandwidth quota for a final packet payload.
    ///
    /// Returns false when this packet cannot currently fit. The caller should stop sending for
    /// this tick, because subsequent packets are not higher priority than this one.
    pub(crate) fn consume_packet_quota(&mut self, packet_bytes: usize) -> bool {
        if !self.enabled {
            return true;
        }

        let Ok(packet_bytes) = u32::try_from(packet_bytes) else {
            error!("packet size exceeds bandwidth limiter range");
            return false;
        };
        let nonzero_packet_bytes = NonZeroU32::try_from(packet_bytes).unwrap();
        let Ok(result) = self.limiter.check_n(nonzero_packet_bytes) else {
            error!("the bandwidth quota cannot fit a packet of size {packet_bytes}");
            return false;
        };
        if result.is_err() {
            debug!("Bandwidth quota reached, no more packets can be sent this tick");
            return false;
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channel::builder::{ChannelMode, ChannelSettings};
    use crate::channel::registry::ChannelKind;
    use crate::packet::message::{
        FragmentCompression, FragmentData, FragmentIndex, MessageData, MessageId, SendCandidate,
        SendMessage, SendMessageKey, SingleData,
    };
    use alloc::vec;
    use bytes::Bytes;

    struct PingLikeChannel;
    struct TurnTrafficChannel;
    struct FragmentChannel;

    #[test]
    fn disabled_priority_uses_fragment_first_size_ascending_packing_order() {
        let registry = ChannelRegistry::default();
        let mut manager = PriorityManager::default();
        manager.candidates_mut().push(SendCandidate::new(
            ChannelKind::of::<TurnTrafficChannel>(),
            10,
            SendMessageKey::UnreliableSingle(0),
            SendMessage {
                priority: 100.0,
                data: SingleData::new(None, Bytes::from_static(b"larger-single")).into(),
            },
        ));
        manager.candidates_mut().push(SendCandidate::new(
            ChannelKind::of::<FragmentChannel>(),
            5,
            SendMessageKey::UnreliableFragment(0),
            SendMessage {
                priority: 0.1,
                data: FragmentData {
                    message_id: MessageId(0),
                    fragment_id: FragmentIndex(0),
                    num_fragments: FragmentIndex(1),
                    compression: Some(FragmentCompression::None),
                    bytes: Bytes::from_static(b"fragment"),
                }
                .into(),
            },
        ));
        manager.candidates_mut().push(SendCandidate::new(
            ChannelKind::of::<PingLikeChannel>(),
            1,
            SendMessageKey::UnreliableSingle(0),
            SendMessage {
                priority: 1.0,
                data: SingleData::new(None, Bytes::from_static(b"small")).into(),
            },
        ));

        manager.prioritize(&registry);

        assert!(matches!(
            manager.candidates()[0].message.data,
            MessageData::Fragment(_)
        ));
        assert_eq!(manager.candidates()[1].channel_id, 1);
        assert_eq!(manager.candidates()[2].channel_id, 10);
    }

    #[test]
    fn infinite_priority_messages_are_ordered_before_traffic_bursts() {
        let mut registry = ChannelRegistry::default();
        let (ping_kind, ping_channel) = registry.add_channel::<PingLikeChannel>(ChannelSettings {
            mode: ChannelMode::UnorderedUnreliable,
            priority: f32::INFINITY,
            ..Default::default()
        });
        let (traffic_kind, traffic_channel) =
            registry.add_channel::<TurnTrafficChannel>(ChannelSettings {
                mode: ChannelMode::UnorderedUnreliable,
                priority: 1.0,
                ..Default::default()
            });
        let mut manager = PriorityManager::new(PriorityConfig::new(1024));

        manager.candidates_mut().extend((0..120).map(|index| {
            SendCandidate::new(
                traffic_kind,
                traffic_channel,
                SendMessageKey::UnreliableSingle(index),
                SendMessage {
                    priority: 1.0,
                    data: SingleData::new(None, Bytes::from(vec![index as u8; 8])).into(),
                },
            )
        }));
        manager.candidates_mut().push(SendCandidate::new(
            ping_kind,
            ping_channel,
            SendMessageKey::UnreliableSingle(0),
            SendMessage {
                priority: 1.0,
                data: SingleData::new(None, Bytes::from_static(b"ping")).into(),
            },
        ));

        manager.prioritize(&registry);

        let candidate = manager.candidates().first().expect("expected a candidate");
        assert_eq!(candidate.channel_id, ping_channel);
        let crate::packet::message::MessageData::Single(message) = &candidate.message.data else {
            panic!("expected a single message")
        };
        assert_eq!(message.bytes, Bytes::from_static(b"ping"));
    }

    #[test]
    fn higher_priority_single_precedes_lower_priority_fragment() {
        let mut registry = ChannelRegistry::default();
        let (fragment_kind, fragment_channel) =
            registry.add_channel::<FragmentChannel>(ChannelSettings {
                mode: ChannelMode::UnorderedUnreliable,
                priority: 1.0,
                ..Default::default()
            });
        let (ping_kind, ping_channel) = registry.add_channel::<PingLikeChannel>(ChannelSettings {
            mode: ChannelMode::UnorderedUnreliable,
            priority: 10.0,
            ..Default::default()
        });
        let mut manager = PriorityManager::new(PriorityConfig::new(1024));
        manager.candidates_mut().push(SendCandidate::new(
            fragment_kind,
            fragment_channel,
            SendMessageKey::UnreliableFragment(0),
            SendMessage {
                data: FragmentData {
                    message_id: MessageId(0),
                    fragment_id: FragmentIndex(0),
                    num_fragments: FragmentIndex(1),
                    compression: Some(FragmentCompression::None),
                    bytes: Bytes::from_static(b"fragment"),
                }
                .into(),
                priority: 1.0,
            },
        ));
        manager.candidates_mut().push(SendCandidate::new(
            ping_kind,
            ping_channel,
            SendMessageKey::UnreliableSingle(0),
            SendMessage {
                data: SingleData::new(None, Bytes::from_static(b"urgent")).into(),
                priority: 1.0,
            },
        ));

        manager.prioritize(&registry);

        assert!(matches!(
            manager.candidates()[0].message.data,
            MessageData::Single(_)
        ));
    }

    #[test]
    fn equal_priority_uses_fragment_first_size_ascending_packing_order() {
        let mut registry = ChannelRegistry::default();
        let (kind, channel) = registry.add_channel::<FragmentChannel>(ChannelSettings {
            mode: ChannelMode::UnorderedUnreliable,
            priority: 1.0,
            ..Default::default()
        });
        let mut manager = PriorityManager::new(PriorityConfig::new(1024));
        manager.candidates_mut().push(SendCandidate::new(
            kind,
            channel,
            SendMessageKey::UnreliableSingle(0),
            SendMessage {
                priority: 1.0,
                data: SingleData::new(None, Bytes::from_static(b"larger-single")).into(),
            },
        ));
        manager.candidates_mut().push(SendCandidate::new(
            kind,
            channel,
            SendMessageKey::UnreliableSingle(1),
            SendMessage {
                priority: 1.0,
                data: SingleData::new(None, Bytes::from_static(b"small")).into(),
            },
        ));
        manager.candidates_mut().push(SendCandidate::new(
            kind,
            channel,
            SendMessageKey::UnreliableFragment(0),
            SendMessage {
                priority: 1.0,
                data: FragmentData {
                    message_id: MessageId(0),
                    fragment_id: FragmentIndex(0),
                    num_fragments: FragmentIndex(1),
                    compression: Some(FragmentCompression::None),
                    bytes: Bytes::from_static(b"fragment"),
                }
                .into(),
            },
        ));

        manager.prioritize(&registry);

        assert!(matches!(
            manager.candidates()[0].message.data,
            MessageData::Fragment(_)
        ));
        let [small, large] = &manager.candidates()[1..] else {
            panic!("expected two single candidates")
        };
        assert!(small.message.data.bytes_len() < large.message.data.bytes_len());
    }

    #[test]
    fn fragment_order_uses_existing_message_and_fragment_ids() {
        let mut registry = ChannelRegistry::default();
        let (kind, channel) = registry.add_channel::<FragmentChannel>(ChannelSettings {
            mode: ChannelMode::UnorderedUnreliable,
            priority: 1.0,
            ..Default::default()
        });
        let mut manager = PriorityManager::new(PriorityConfig::new(1024));
        for (queue_index, message_id, fragment_id) in [(3, 1, 1), (1, 0, 1), (2, 1, 0), (0, 0, 0)] {
            manager.candidates_mut().push(SendCandidate::new(
                kind,
                channel,
                SendMessageKey::UnreliableFragment(queue_index),
                SendMessage {
                    priority: 1.0,
                    data: FragmentData {
                        message_id: MessageId(message_id),
                        fragment_id: FragmentIndex(fragment_id),
                        num_fragments: FragmentIndex(2),
                        compression: (fragment_id == 0).then_some(FragmentCompression::None),
                        bytes: Bytes::new(),
                    }
                    .into(),
                },
            ));
        }

        manager.prioritize(&registry);

        let order = manager
            .candidates()
            .iter()
            .map(|candidate| {
                let MessageData::Fragment(fragment) = &candidate.message.data else {
                    panic!("expected fragment candidate")
                };
                (fragment.message_id.0, fragment.fragment_id.0)
            })
            .collect::<Vec<_>>();
        assert_eq!(order, [(0, 0), (0, 1), (1, 0), (1, 1)]);
    }

    #[test]
    fn bandwidth_burst_is_independent_of_mtu() {
        let mut limiter = BandwidthLimiter::new(PriorityConfig::new(1).with_burst_size(300));

        assert!(limiter.consume_packet_quota(300));
        assert!(!limiter.consume_packet_quota(301));
    }
}
