use alloc::collections::VecDeque;
use alloc::{vec, vec::Vec};
use bevy_platform::collections::HashMap;
use core::num::NonZeroU32;

use crate::channel::ChannelKind;
use crate::channel::builder::SenderMetadata;
use crate::channel::registry::{ChannelId, ChannelRegistry};
use crate::packet::message::{FragmentData, MessageData, SendMessage, SingleData};
use governor::{DefaultDirectRateLimiter, Quota};
use lightyear_core::network::NetId;
use lightyear_serde::ToBytes;
use nonzero_ext::*;
#[cfg(feature = "trace")]
use tracing::{Level, instrument};
#[allow(unused_imports)]
use tracing::{debug, error, info, trace};

const BYPASS_QUOTA_PRIORITY: f32 = 100000.0;

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

    // TODO: maybe accumulate the used_bytes in the priority_manager instead of returning here?
    /// Filter the messages by priority and bandwidth quota
    /// Returns the list of messages that we can send, along with the amount of bytes we used
    /// in the rate limiter.
    #[cfg_attr(feature = "trace", instrument(level = Level::INFO, skip_all))]
    pub(crate) fn priority_filter(
        &mut self,
        channel_registry: &ChannelRegistry,
        senders: &mut HashMap<ChannelKind, SenderMetadata>,
    ) -> (
        Vec<(ChannelId, VecDeque<SingleData>)>,
        Vec<(ChannelId, VecDeque<FragmentData>)>,
        u32,
    ) {
        // if the bandwidth quota is disabled, just pass all messages through
        // As an optimization: no need to send the tick of the message, it is the same as the header tick
        if !self.config.enabled {
            let mut single_data = vec![];
            let mut fragment_data = vec![];
            for (net_id, (single, fragment)) in self.data_to_send.drain(..) {
                #[cfg(feature = "metrics")]
                let channel_name = channel_registry.get_name_from_net_id(net_id);
                #[cfg(feature = "metrics")]
                metrics::gauge!("channel/send_messages", "channel" => channel_name)
                    .increment((single.len() + fragment.len()) as f64);

                single_data.push((
                    net_id,
                    single
                        .into_iter()
                        .map(|message| {
                            let MessageData::Single(single) = message.data else {
                                unreachable!()
                            };
                            #[cfg(feature = "metrics")]
                            {
                                metrics::gauge!("channel/send_bytes", "channel" => channel_name)
                                    .increment(single.bytes_len() as f64);
                            }
                            // we don't actually use the `messages_sent` field when the priority filter is disabled
                            // but we still include it so that we can easily check in tests how many messages were sent
                            #[cfg(feature = "test_utils")]
                            if let Some(message_id) = single.id {
                                let channel_kind =
                                    channel_registry.get_kind_from_net_id(net_id).unwrap();
                                senders
                                    .get_mut(channel_kind)
                                    .unwrap()
                                    .messages_sent
                                    .push(message_id);
                            }
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
                            #[cfg(feature = "metrics")]
                            {
                                metrics::gauge!("channel/send_bytes", "channel" => channel_name)
                                    .increment(fragment.bytes_len() as f64);
                            }
                            fragment
                        })
                        .collect(),
                ));
            }
            return (single_data, fragment_data, 0);
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

        // select the top messages with the rate limiter
        let mut single_data: HashMap<ChannelId, VecDeque<SingleData>> = HashMap::default();
        let mut fragment_data: HashMap<ChannelId, VecDeque<FragmentData>> = HashMap::default();
        let mut bytes_used = 0;
        while let Some(buffered_message) = all_messages.pop() {
            // we don't use the exact size of the message, but the size of the bytes
            // we will adjust for this later
            let message_bytes = buffered_message.data.bytes_len() as u32;
            let nonzero_message_bytes = NonZeroU32::try_from(message_bytes).unwrap();
            let Ok(result) = self.limiter.check_n(nonzero_message_bytes) else {
                error!("the bandwidth does not have enough capacity for a message of this size!");
                break;
            };

            // above BYPASS_QUOTA_PRIORITY, we still send the message
            if buffered_message.priority < BYPASS_QUOTA_PRIORITY {
                let Ok(()) = result else {
                    debug!("Bandwidth quota reached, no more messages can be sent this tick");
                    break;
                };
            }
            trace!(channel=?buffered_message.channel_net_id, "Sending message with priority {:?}", buffered_message.priority);

            // keep track of the bytes we added to the rate limiter
            bytes_used += message_bytes;

            // notify the replication sender that the message was actually sent
            if let Some(message_id) = buffered_message.data.message_id() {
                let channel_kind = channel_registry
                    .get_kind_from_net_id(buffered_message.channel_net_id)
                    .unwrap();
                senders
                    .get_mut(channel_kind)
                    .unwrap()
                    .messages_sent
                    .push(message_id);
            }

            #[cfg(feature = "metrics")]
            {
                let channel_name =
                    channel_registry.get_name_from_net_id(buffered_message.channel_net_id);
                metrics::gauge!("channel/send_messages", "channel" => channel_name).increment(1);
                metrics::gauge!("channel/send_bytes", "channel" => channel_name)
                    .increment(buffered_message.data.bytes_len() as f64);
            }

            // the message is allowed, add it to the list of messages to send
            match buffered_message.data {
                MessageData::Single(single) => {
                    single_data
                        .entry(buffered_message.channel_net_id)
                        .or_default()
                        .push_back(single);
                }
                MessageData::Fragment(fragment) => {
                    fragment_data
                        .entry(buffered_message.channel_net_id)
                        .or_default()
                        .push_back(fragment);
                }
            }
        }

        // all the other messages that don't make the cut, we just drop
        // - unreliable messages: they are unreliable so it's ok
        // - reliable messages: they will be retried later, maybe with higher priority?
        // - unreliable entity updates: the replication sender keeps track for each entity of when we were able to send an update
        //   - PROBLEM: we could have the entity action not get sent (bandwidth), and then the priority still drops because the entity update
        //     was sent right after...
        // - reliable entity actions:
        let num_messages_sent = single_data.values().map(|data| data.len()).sum::<usize>()
            + fragment_data.values().map(|data| data.len()).sum::<usize>();
        debug!(
            bytes_sent = ?bytes_used,
            ?num_messages_sent,
            num_messages_discarded = ?all_messages.len(),
            "priority filter done.");

        self.data_to_send.clear();
        (
            single_data.into_iter().collect(),
            fragment_data.into_iter().collect(),
            bytes_used,
        )
    }
}
