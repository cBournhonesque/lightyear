use bevy::platform::collections::HashMap;
use alloc::collections::VecDeque;
#[cfg(not(feature = "std"))]
use alloc::{vec, vec::Vec};
use core::num::NonZeroU32;

use crossbeam_channel::{Receiver, Sender};
use governor::{DefaultDirectRateLimiter, Quota};
use nonzero_ext::*;
use tracing::{debug, error, trace};
#[cfg(feature = "trace")]
use tracing::{instrument, Level};

use crate::packet::message::{FragmentData, MessageData, MessageId, SendMessage, SingleData};
use crate::prelude::{ChannelRegistry, Tick};
use crate::protocol::channel::ChannelId;
use crate::protocol::registry::NetId;

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

impl From<crate::client::config::PacketConfig> for PriorityConfig {
    fn from(value: crate::client::config::PacketConfig) -> Self {
        Self {
            bandwidth_quota: value.send_bandwidth_cap,
            enabled: value.bandwidth_cap_enabled,
        }
    }
}

impl From<crate::server::config::PacketConfig> for PriorityConfig {
    fn from(value: crate::server::config::PacketConfig) -> Self {
        Self {
            bandwidth_quota: value.per_client_send_bandwidth_cap,
            enabled: value.bandwidth_cap_enabled,
        }
    }
}

#[derive(Debug)]
pub(crate) struct PriorityManager {
    pub(crate) config: PriorityConfig,
    // TODO: can I do without this limiter?
    pub(crate) limiter: DefaultDirectRateLimiter,
    // // Internal buffer of data that we want to send
    // // Reuse allocation across frames
    // data_to_send: BTreeMap<ChannelId, (VecDeque<SendMessage>, VecDeque<SendMessage>)>,
    // Messages that could not be sent because of the bandwidth quota
    // buffered_data: Vec<BufferedMessage>,
    /// List of senders to notify when a replication update message is actually sent (included in packet)
    replication_update_senders: Vec<Sender<MessageId>>,
}

impl PriorityManager {
    pub(crate) fn new(config: PriorityConfig) -> Self {
        Self {
            config: config.clone(),
            limiter: DefaultDirectRateLimiter::direct(config.bandwidth_quota),
            // data_to_send: BTreeMap::new(),
            // buffered_data: Vec::new(),
            replication_update_senders: Vec::new(),
        }
    }

    /// Create a channel to notify when a replication update message is actually sent (included in packet)
    /// (as opposed to dropped because of the bandwidth quota)
    pub(crate) fn subscribe_replication_update_sent_messages(&mut self) -> Receiver<MessageId> {
        let (sender, receiver) = crossbeam_channel::unbounded();
        self.replication_update_senders.push(sender);
        receiver
    }

    // TODO: maybe accumulate the used_bytes in the priority_manager instead of returning here?
    /// Filter the messages by priority and bandwidth quota
    /// Returns the list of messages that we can send, along with the amount of bytes we used
    /// in the rate limiter.
    #[cfg_attr(feature = "trace", instrument(level = Level::INFO, skip_all))]
    pub(crate) fn priority_filter(
        &mut self,
        data: Vec<(ChannelId, (VecDeque<SendMessage>, VecDeque<SendMessage>))>,
        channel_registry: &ChannelRegistry,
        tick: Tick,
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
            for (net_id, (single, fragment)) in data {
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
            return (single_data, fragment_data, 0);
        }

        // compute the priority of each new message
        let mut all_messages = data
            .into_iter()
            .flat_map(|(net_id, (single, fragment))| {
                let channel_priority = channel_registry
                    .get_builder_from_net_id(net_id)
                    .unwrap()
                    .settings
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
            if channel_registry.is_replication_update_channel(buffered_message.channel_net_id) {
                // SAFETY: we are guaranteed in this situation to have a message id (because we use the unreliable with acks sender)
                let message_id = buffered_message.data.message_id().unwrap();
                for sender in self.replication_update_senders.iter() {
                    trace!(
                        ?message_id,
                        "notifying replication sender that a message was actually sent."
                    );
                    let _ = sender.send(message_id).map_err(|e| {
                        error!("error notifying replication sender that a message was actually sent: {:?}", e)
                    });
                }
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

        (
            single_data.into_iter().collect(),
            fragment_data.into_iter().collect(),
            bytes_used,
        )
    }
}
