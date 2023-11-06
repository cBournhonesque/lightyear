use crate::tick::ping_store::PingId;
use crate::tick::Tick;
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

#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum SyncMessage {
    Ping(PingMessage),
    Pong(PongMessage),
}
