use crate::channel::registry::{ChannelId, ChannelRegistry};
use crate::packet::message::{FragmentData, MessageData, SendMessage, SingleData};
use alloc::collections::VecDeque;
use alloc::{vec, vec::Vec};
use core::num::NonZeroU32;
use governor::{DefaultDirectRateLimiter, Quota};
use lightyear_core::network::NetId;
use nonzero_ext::*;
#[cfg(feature = "trace")]
use tracing::{Level, instrument};
#[allow(unused_imports)]
use tracing::{debug, error, info, trace};

#[derive(Debug)]
pub struct BufferedMessage {
    priority: f32,
    channel_net_id: NetId,
    data: MessageData,
}

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
    pub fn new(bytes_per_second_quota: u32) -> Self {
        let cap = bytes_per_second_quota.try_into().unwrap();
        Self {
            bandwidth_quota: Quota::per_second(cap).allow_burst(cap),
            enabled: true,
        }
    }
}

/// Responsible for restricting the bandwidth used by messages sent over the network.
///
/// The messages will be filtered by priority until we reach the bandwidth quota.
///
/// Messages that were not sent will have increased priority, which means that they have a higher chance of
/// being sent in the future.
#[derive(Debug)]
pub struct PriorityManager {
    pub(crate) config: PriorityConfig,
    // TODO: can I do without this limiter?
    pub(crate) limiter: DefaultDirectRateLimiter,
    // Internal buffer of data that we want to send
    // TODO: improve this
    data_to_send: Vec<(NetId, (VecDeque<SendMessage>, VecDeque<SendMessage>))>,
    // Messages that could not be sent because of the bandwidth quota
    // buffered_data: Vec<BufferedMessage>,
}

impl Default for PriorityManager {
    fn default() -> Self {
        Self::new(PriorityConfig::default())
    }
}

impl PriorityManager {
    pub fn new(config: PriorityConfig) -> Self {
        let quota = config.bandwidth_quota;
        Self {
            config,
            data_to_send: Vec::new(),
            limiter: DefaultDirectRateLimiter::direct(quota),
            // data_to_send: BTreeMap::new(),
            // buffered_data: Vec::new(),
        }
    }

    pub(crate) fn buffer_messages(
        &mut self,
        net_id: NetId,
        single: VecDeque<SendMessage>,
        fragment: VecDeque<SendMessage>,
    ) {
        self.data_to_send.push((net_id, (single, fragment)));
    }

    /// Sort queued messages by priority and return the data to packetize.
    ///
    /// Bandwidth quota is intentionally consumed after packet building, because packet building
    /// can compress selected messages and the limiter should use final packet bytes.
    #[cfg_attr(feature = "trace", instrument(level = Level::INFO, skip_all))]
    pub(crate) fn prioritize(
        &mut self,
        channel_registry: &ChannelRegistry,
    ) -> (
        Vec<(ChannelId, VecDeque<SingleData>)>,
        Vec<(ChannelId, VecDeque<FragmentData>)>,
    ) {
        // if the bandwidth quota is disabled, just pass all messages through
        // As an optimization: no need to send the tick of the message, it is the same as the header tick
        if !self.config.enabled {
            let mut single_data = vec![];
            let mut fragment_data = vec![];
            for (net_id, (single, fragment)) in self.data_to_send.drain(..) {
                single_data.push((
                    net_id,
                    single
                        .into_iter()
                        .map(|message| {
                            let MessageData::Single(single) = message.data else {
                                unreachable!()
                            };
                            single
                        })
                        .collect(),
                ));
                fragment_data.push((
                    net_id,
                    fragment
                        .into_iter()
                        .map(|message| {
                            let MessageData::Fragment(fragment) = message.data else {
                                unreachable!()
                            };
                            fragment
                        })
                        .collect(),
                ));
            }
            return (single_data, fragment_data);
        }

        // compute the priority of each new message
        let mut all_messages = self
            .data_to_send
            .drain(..)
            .flat_map(|(net_id, (single, fragment))| {
                let channel_priority = channel_registry
                    .settings_from_net_id(net_id)
                    .unwrap()
                    .priority;
                trace!(?channel_priority, num_single=?single.len(), "channel priority");
                single
                    .into_iter()
                    .map(move |single| BufferedMessage {
                        priority: single.priority * channel_priority,
                        channel_net_id: net_id,
                        data: single.data,
                    })
                    .chain(fragment.into_iter().map(move |fragment| {
                        // TODO (IMPORTANT): we should split fragments AFTER priority filtering
                        //  because if we don't send one fragment, it's over..
                        BufferedMessage {
                            priority: fragment.priority * channel_priority,
                            channel_net_id: net_id,
                            data: fragment.data,
                        }
                    }))
            })
            .collect::<Vec<_>>();

        // sort from highest priority to lower
        all_messages.sort_by(|a, b| a.priority.partial_cmp(&b.priority).unwrap());
        debug!(
            "all messages to send, sorted by priority: {:?}",
            all_messages
        );

        // Return messages in priority order. Keep each message as its own channel batch so the
        // packet builder sees priority ordering instead of a grouping hash-map order.
        let mut single_data: Vec<(ChannelId, VecDeque<SingleData>)> = vec![];
        let mut fragment_data: Vec<(ChannelId, VecDeque<FragmentData>)> = vec![];
        while let Some(buffered_message) = all_messages.pop() {
            trace!(channel=?buffered_message.channel_net_id, "Selected message with priority {:?}", buffered_message.priority);

            // the message is allowed, add it to the list of messages to send
            match buffered_message.data {
                MessageData::Single(single) => {
                    single_data.push((buffered_message.channel_net_id, VecDeque::from([single])));
                }
                MessageData::Fragment(fragment) => {
                    fragment_data
                        .push((buffered_message.channel_net_id, VecDeque::from([fragment])));
                }
            }
        }

        let num_messages_sent = single_data
            .iter()
            .map(|(_, data)| data.len())
            .sum::<usize>()
            + fragment_data
                .iter()
                .map(|(_, data)| data.len())
                .sum::<usize>();
        debug!(?num_messages_sent, "priority ordering complete.");

        self.data_to_send.clear();
        (single_data, fragment_data)
    }

    /// Consume bandwidth quota for a final packet payload.
    ///
    /// Returns false when this packet cannot currently fit. The caller should stop sending for
    /// this tick, because subsequent packets are not higher priority than this one.
    pub(crate) fn consume_packet_quota(&mut self, packet_bytes: u32) -> bool {
        if !self.config.enabled {
            return true;
        }

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
