use crate::ping_manager::{PingConfig, PingManager};
use crate::sync::SyncManager;
use crate::tick_manager::TickManager;
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

    // pub(crate) ping_manager: PingManager,
    pub(crate) sync_manager: SyncManager,
    // TODO: maybe don't do any replication until connection is synced?
}

impl<P: Protocol> Connection<P> {
    pub fn new(channel_registry: &ChannelRegistry, ping_config: &PingConfig) -> Self {
        Self {
            base: lightyear_shared::Connection::new(channel_registry),
            // ping_manager: PingManager::new(ping_config),
            sync_manager: SyncManager::new(10, Duration::from_millis(50)),
        }
    }

    pub fn update(
        &mut self,
        delta: Duration,
        time_manager: &TimeManager,
        tick_manager: &TickManager,
    ) {
        self.base.update(delta);
        // TODO: maybe prepare ping?
        // self.ping_manager.update(delta);

        // if not synced, keep doing syncing
        if !self.sync_manager.is_synced() {
            if let Some(sync_ping) = self
                .sync_manager
                .maybe_prepare_ping(time_manager, tick_manager)
            {
                let message = ProtocolMessage::Sync(SyncMessage::TimeSyncPing(sync_ping));
                let channel = ChannelKind::of::<DefaultUnreliableChannel>();
                self.base.message_manager.buffer_send(message, channel)
            }
        }
    }

    // pub fn buffer_ping(&mut self, time_manager: &TimeManager) -> Result<()> {
    //     if !self.ping_manager.should_send_ping() {
    //         return Ok(());
    //     }
    //
    //     let ping_message = self.ping_manager.prepare_ping(time_manager);
    //
    //     // info!("Sending ping {:?}", ping_message);
    //     trace!("Sending ping {:?}", ping_message);
    //
    //     let message = ProtocolMessage::Sync(SyncMessage::Ping(ping_message));
    //     let channel = ChannelKind::of::<DefaultUnreliableChannel>();
    //     self.base.message_manager.buffer_send(message, channel)
    // }

    // TODO: eventually call handle_ping and handle_pong directly from the connection
    //  without having to send to events

    // send pongs for every ping we received
    // pub fn buffer_pong(&mut self, time_manager: &TimeManager, ping: PingMessage) -> Result<()> {
    //     let pong_message = self.ping_manager.prepare_pong(time_manager, ping);
    //
    //     // info!("Sending ping {:?}", ping_message);
    //     trace!("Sending pong {:?}", pong_message);
    //     let message = ProtocolMessage::Sync(SyncMessage::Pong(pong_message));
    //     let channel = ChannelKind::of::<DefaultUnreliableChannel>();
    //     self.base.message_manager.buffer_send(message, channel)
    // }
}
