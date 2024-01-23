//! Handles setting and updating the priority for each replicated entity

// TODO:
// - after replicate is added, if bandwidth_cap is enabled, set the priority on each entity.
// - or maybe we just need to keep track of the priorities in the send_manager.

use crate::packet::message::MessageId;
use crossbeam_channel::Receiver;

pub struct PriorityManager {
    /// Get notified whenever a message for a given ReplicationGroup was actually sent
    /// (sometimes they might not be sent because of bandwidth constraints
    pub message_send_receiver: Receiver<MessageId>,
}
