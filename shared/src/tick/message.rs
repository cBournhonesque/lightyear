use bitcode::{Decode, Encode};
use lightyear_derive::MessageInternal;
use serde::{Deserialize, Serialize};

use crate::tick::ping_store::PingId;
use crate::tick::Tick;
use crate::WrappedTime;

#[derive(Encode, Decode, Serialize, Deserialize, Clone, Debug)]
pub struct PingMessage {
    pub id: PingId,
    // tick of the host
    pub tick: Tick,
}

impl PingMessage {
    pub fn new(id: PingId, tick: Tick) -> Self {
        Self { id, tick }
    }
}

// TODO: could distinguish between sync and simple pong?
#[derive(Encode, Decode, Serialize, Deserialize, Clone, Debug)]
pub struct PongMessage {
    /// id of the ping message that triggered this pong
    pub ping_id: PingId,

    // tick of the host when the message was emitted
    pub tick: Tick,
    // if positive, the remote (client) is ahead of the host (server)
    // else it's behind the server
    pub offset_sec: f64,
}

// ping sent from client to server to establish time sync
#[derive(Encode, Decode, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct TimeSyncPingMessage {
    pub id: PingId,
    // tick of the host
    pub tick: Tick,
    // time when the server received the ping
    pub ping_received_time: Option<WrappedTime>,
}

/// pong sent from server to client to establish time sync
#[derive(Encode, Decode, Serialize, Deserialize, Clone, Debug)]
pub struct TimeSyncPongMessage {
    /// id of the ping message that triggered this pong
    pub ping_id: PingId,
    // TODO: these two fields should not be here, because they will be in header

    // RELATED TO TICKS
    // /// time where the server switched to the current tick
    // pub server_tick_instant: WrappedTime,
    // /// current server tick
    // pub server_tick: Tick,

    // COMPUTE RTT/OFFSET
    /// time where the server received the ping
    pub ping_received_time: WrappedTime,
    /// time when the server sent the pong
    pub pong_sent_time: WrappedTime,
    // #[bitcode_hint(expected_range = "0.0..1.0")]
    // pub tick_duration_ms_avg: f32,
    // pub tick_speedup_potential: f32,
}

#[derive(MessageInternal, Encode, Decode, Serialize, Deserialize, Clone, Debug)]
#[serialize(bitcode)]
pub enum SyncMessage {
    Ping(PingMessage),
    Pong(PongMessage),
    TimeSyncPing(TimeSyncPingMessage),
    TimeSyncPong(TimeSyncPongMessage),
}
