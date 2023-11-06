use crate::time::TimeManager;
use lightyear_shared::channel::channel::DefaultUnreliableChannel;
use lightyear_shared::connection::ProtocolMessage;
use lightyear_shared::transport::PacketReceiver;
use lightyear_shared::{
    ChannelKind, Connection, MessageManager, PingMessage, PingStore, Protocol, SyncMessage,
    TickManager,
};

/// Data structure for managing synchronization of the client ticks with the server
pub struct SyncManager {
    time_manager: TimeManager,
    ping_store: PingStore,
}

impl SyncManager {
    pub fn new(time_manager: TimeManager) -> Self {
        Self {
            time_manager,
            ping_store: PingStore::new(),
        }
    }

    /// Send a ping to the server
    pub fn send_ping(&mut self) -> PingMessage {
        let ping_id = self.ping_store.push_new(self.time_manager.current_time());
        PingMessage {
            id: ping_id,
            tick: self.time_manager.current_tick(),
        }
        // let message = ProtocolMessage::Sync(SyncMessage::Ping(ping));
        // let channel = ChannelKind::of::<DefaultUnreliableChannel>();
        // connection.message_manager.buffer_send(message, channel)
    }

    pub fn handle_pong(&mut self) {}
}
