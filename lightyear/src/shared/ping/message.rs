//! Defines the actual ping/pong messages
use bitcode::{Decode, Encode};

use crate::shared::ping::store::PingId;
use crate::shared::time_manager::WrappedTime;

// TODO: do we need the ping ids? we could just re-use the message id ?
/// Ping message; the remote should respond immediately with a pong
#[derive(Encode, Decode, Clone, Debug, PartialEq)]
pub struct Ping {
    pub id: PingId,
}

/// Pong message sent in response to a ping
#[derive(Encode, Decode, Clone, Debug)]
pub struct Pong {
    /// id of the ping message that triggered this pong
    pub ping_id: PingId,
    /// time when the ping was received
    pub ping_received_time: WrappedTime,
    /// time when the pong was sent
    pub pong_sent_time: WrappedTime,
}

#[derive(Encode, Decode, Clone, Debug)]
pub enum SyncMessage {
    Ping(Ping),
    Pong(Pong),
}
