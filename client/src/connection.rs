use crate::ping_manager::PingConfig;
use crate::sync::SyncManager;
use anyhow::Result;
use lightyear_shared::connection::ProtocolMessage;
use lightyear_shared::tick::Tick;
use lightyear_shared::{
    ChannelKind, ChannelRegistry, DefaultSequencedUnreliableChannel, PingMessage, Protocol,
    ReadBuffer, SyncMessage, TickManager, TimeManager, World,
};
use std::time::Duration;
use tracing::{debug, info, trace};

// TODO: this layer of indirection is annoying, is there a better way?
//  maybe just pass the inner connection to ping_manager? (but harder to test)
pub struct Connection<P: Protocol> {
    pub(crate) base: lightyear_shared::Connection<P>,

    // pub(crate) ping_manager: PingManager,
    pub(crate) sync_manager: SyncManager,
    // TODO: see if this is correct; should we instead attach the tick on every update message?
    /// Tick of the server that we last received in any packet from the server.
    /// This is not updated every tick!
    pub(crate) latest_received_server_tick: Tick,
    // TODO: maybe don't do any replication until connection is synced?
}

impl<P: Protocol> Connection<P> {
    pub fn new(channel_registry: &ChannelRegistry, ping_config: &PingConfig) -> Self {
        Self {
            base: lightyear_shared::Connection::new(channel_registry),
            // ping_manager: PingManager::new(ping_config),
            sync_manager: SyncManager::new(
                ping_config.sync_num_pings,
                ping_config.sync_ping_interval_ms,
            ),
            latest_received_server_tick: Tick(0),
        }
    }

    pub fn update(
        &mut self,
        delta: Duration,
        time_manager: &TimeManager,
        tick_manager: &TickManager,
    ) {
        self.base.update(delta, tick_manager);
        self.sync_manager.update(delta);
        // TODO: maybe prepare ping?
        // self.ping_manager.update(delta);

        // if not synced, keep doing syncing
        if !self.sync_manager.is_synced() {
            if let Some(sync_ping) = self
                .sync_manager
                .maybe_prepare_ping(time_manager, tick_manager)
            {
                let message = ProtocolMessage::Sync(SyncMessage::TimeSyncPing(sync_ping));
                let channel = ChannelKind::of::<DefaultSequencedUnreliableChannel>();
                self.base
                    .message_manager
                    .buffer_send(message, channel)
                    .unwrap();
            }
        }
    }

    pub fn recv_packet(&mut self, reader: &mut impl ReadBuffer) -> Result<()> {
        let tick = self.base.recv_packet(reader)?;
        if tick > self.latest_received_server_tick {
            self.latest_received_server_tick = tick;
        }
        Ok(())
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
