use serde::{Deserialize, Serialize};

use crate::shared::ping::store::PingId;
use crate::shared::tick_manager::Tick;
use crate::shared::time_manager::WrappedTime;

/// Ping message; the remote should response immediately with a pong
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Ping {
    pub id: PingId,
}

/// Pong message sent in response to a ping
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Pong {
    /// id of the ping message that triggered this pong
    pub ping_id: PingId,
    /// time when the ping was received
    pub ping_received_time: WrappedTime,
    /// time when the pong was sent
    pub pong_sent_time: WrappedTime,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum SyncMessage {
    Ping(Ping),
    Pong(Pong),
}
