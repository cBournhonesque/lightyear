use crate::tick::ping_store::PingId;
use crate::tick::Tick;
use crate::WrappedTime;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug)]
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
#[derive(Serialize, Deserialize, Clone, Debug)]
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
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct TimeSyncPingMessage {
    pub id: PingId,
    // tick of the host
    pub tick: Tick,
}

/// pong sent from server to client to establish time sync
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct TimeSyncPongMessage {
    /// id of the ping message that triggered this pong
    pub ping_id: PingId,
    /// time where the server switched to the current tick
    pub last_tick_time: WrappedTime,
    /// current server tick
    pub current_tick: Tick,
    /// time where the server received the ping
    pub ping_received_time: WrappedTime,
    /// time when the server sent the pong
    pub pong_sent_time: WrappedTime,

    // TODO: tick duration avg
    // TODO: tick speedup potential
    // if positive, the remote (client) is ahead of the host (server)
    // else it's behind the server
    pub offset_sec: f64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum SyncMessage {
    Ping(PingMessage),
    Pong(PongMessage),
    TimeSyncPing(TimeSyncPingMessage),
    TimeSyncPong(TimeSyncPongMessage),
}
