use std::collections::{BTreeMap, VecDeque};
use std::num::NonZeroU32;

use crossbeam_channel::{Receiver, Sender};
use governor::{DefaultDirectRateLimiter, Quota};
use nonzero_ext::*;
use tracing::{debug, error, trace};

use crate::_reexport::EntityUpdatesChannel;
use crate::packet::message::{FragmentData, MessageContainer, MessageId, SingleData};
use crate::prelude::{ChannelKind, ChannelRegistry, Tick};
use crate::protocol::registry::NetId;

#[derive(Debug)]
pub struct BufferedMessage {
    priority: f32,
    channel_net_id: NetId,
    message_container: MessageContainer,
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

pub(crate) struct PriorityManager {
    pub(crate) config: PriorityConfig,
    // TODO: can I do without this limiter?
    pub(crate) limiter: DefaultDirectRateLimiter,
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

    // TODO: maybe accumulat ethe used_bytes in the priority_manager instead of returning here?
    /// Filter the messages by priority and bandwidth quota
    /// Returns the list of messages that we can send, along with the amount of bytes we used
    /// in the rate limiter.
    pub(crate) fn priority_filter(
        &mut self,
        data: Vec<(NetId, (VecDeque<SingleData>, VecDeque<FragmentData>))>,
        channel_registry: &ChannelRegistry,
        tick: Tick,
    ) -> (
        BTreeMap<NetId, (VecDeque<SingleData>, VecDeque<FragmentData>)>,
        u32,
    ) {
        // if the bandwidth quota is disabled, just pass all messages through
        // As an optimization: no need to send the tick of the message, it is the same as the header tick
        if !self.config.enabled {
            let mut data_to_send: BTreeMap<NetId, (VecDeque<SingleData>, VecDeque<FragmentData>)> =
                BTreeMap::new();
            for (net_id, (single, fragment)) in data {
                data_to_send.insert(net_id, (single, fragment));
            }
            return (data_to_send, 0);
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
                    .map(move |mut single| {
                        // TODO: this only needs to be done for the messages that are not sent!
                        //  (and for messages that are not replication messages?)
                        // set the initial send tick of the message
                        // we do this because the receiver needs to know at which tick the message was intended to be sent
                        // (for example which tick the EntityAction corresponds to), not the tick of the packet header
                        // when the message was actually sent, which could be later because of bandwidth quota
                        if single.tick.is_none() {
                            single.tick = Some(tick);
                        }
                        BufferedMessage {
                            priority: single.priority * channel_priority,
                            channel_net_id: net_id,
                            message_container: MessageContainer::Single(single),
                        }
                    })
                    .chain(fragment.into_iter().map(move |mut fragment| {
                        if fragment.tick.is_none() {
                            fragment.tick = Some(tick);
                        }
                        BufferedMessage {
                            priority: fragment.priority * channel_priority,
                            channel_net_id: net_id,
                            message_container: MessageContainer::Fragment(fragment),
                        }
                    }))
            })
            .collect::<Vec<_>>();
        // // NOTE: maybe we cannot do this, because for some channels we need to know
        // //  if the message was actually sent or not? (reliable channels)
        // // add all new messages to the list of messages that could not be sent
        // self.buffered_data.extend(all_messages);

        // sort from highest priority to lower
        // self.buffered_data
        all_messages.sort_by(|a, b| a.priority.partial_cmp(&b.priority).unwrap());
        trace!(
            "all messages to send, sorted by priority: {:?}",
            all_messages
        );

        // select the top messages with the rate limiter
        let mut data_to_send: BTreeMap<NetId, (VecDeque<SingleData>, VecDeque<FragmentData>)> =
            BTreeMap::new();
        let mut bytes_used = 0;
        while let Some(buffered_message) = all_messages.pop() {
            trace!(channel=?buffered_message.channel_net_id, "Sending message with priority {:?}", buffered_message.priority);
            // we don't use the exact size of the message, but the size of the bytes
            // we will adjust for this later
            let message_bytes = buffered_message.message_container.bytes().len() as u32;
            let nonzero_message_bytes = NonZeroU32::try_from(message_bytes).unwrap();
            let Ok(result) = self.limiter.check_n(nonzero_message_bytes) else {
                error!("the bandwidth does not have enough capacity for a message of this size!");
                break;
            };
            let Ok(()) = result else {
                debug!("Bandwidth quota reached, no more messages can be sent this tick");
                break;
            };

            // keep track of the bytes we added to the rate limiter
            bytes_used += message_bytes;

            // the message is allowed, add it to the list of messages to send
            let channel_data = data_to_send
                .entry(buffered_message.channel_net_id)
                .or_insert((VecDeque::new(), VecDeque::new()));

            // notify the replication sender that the message was actually sent
            let channel_kind = channel_registry
                .get_kind_from_net_id(buffered_message.channel_net_id)
                .unwrap();
            if channel_kind == &ChannelKind::of::<EntityUpdatesChannel>()
                || channel_kind == &ChannelKind::of::<EntityUpdatesChannel>()
            {
                // SAFETY: we are guaranteed in this situation to have a message id (because we use the unreliable with acks sender)
                let message_id = buffered_message.message_container.message_id().unwrap();
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
            match buffered_message.message_container {
                MessageContainer::Single(single) => {
                    channel_data.0.push_back(single);
                }
                MessageContainer::Fragment(fragment) => {
                    channel_data.1.push_back(fragment);
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
        let num_messages_sent = data_to_send
            .values()
            .map(|(single, fragment)| single.len() + fragment.len())
            .sum::<usize>();
        debug!(
            bytes_sent = ?bytes_used,
            ?num_messages_sent,
            num_messages_discarded = ?all_messages.len(),
            "priority filter done.");

        (data_to_send, bytes_used)
    }
}
