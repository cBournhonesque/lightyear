use crate::ping_manager::{PingConfig, PingManager};
use anyhow::Result;
use lightyear_shared::connection::ProtocolMessage;
use lightyear_shared::{
    ChannelKind, ChannelRegistry, DefaultUnreliableChannel, PingMessage, Protocol, SyncMessage,
    TimeManager,
};
use std::time::Duration;
use tracing::{debug, info, trace};

// TODO: this layer of indirection is annoying, is there a better way?
//  maybe just pass the inner connection to ping_manager? (but harder to test)
pub struct Connection<P: Protocol> {
    pub(crate) base: lightyear_shared::Connection<P>,

    pub(crate) ping_manager: PingManager,
}

impl<P: Protocol> Connection<P> {
    pub fn new(channel_registry: &ChannelRegistry, ping_config: &PingConfig) -> Self {
        Self {
            base: lightyear_shared::Connection::new(channel_registry),
            ping_manager: PingManager::new(ping_config),
        }
    }

    pub fn update(&mut self, delta: Duration) {
        self.base.update(delta);
        self.ping_manager.update(delta);
    }

    // TODO: rename to maybe send ping?
    pub fn buffer_ping(&mut self, time_manager: &TimeManager) -> Result<()> {
        if !self.ping_manager.should_send_ping() {
            return Ok(());
        }

        let ping_message = self.ping_manager.prepare_ping(time_manager);

        // info!("Sending ping {:?}", ping_message);
        trace!("Sending ping {:?}", ping_message);

        let message = ProtocolMessage::Sync(SyncMessage::Ping(ping_message));
        let channel = ChannelKind::of::<DefaultUnreliableChannel>();
        self.base.message_manager.buffer_send(message, channel)
    }

    pub fn buffer_pong(&mut self, time_manager: &TimeManager, ping: PingMessage) -> Result<()> {
        let pong_message = self.ping_manager.prepare_pong(time_manager, ping);

        // info!("Sending ping {:?}", ping_message);
        trace!("Sending pong {:?}", pong_message);
        let message = ProtocolMessage::Sync(SyncMessage::Pong(pong_message));
        let channel = ChannelKind::of::<DefaultUnreliableChannel>();
        self.base.message_manager.buffer_send(message, channel)
    }
}
