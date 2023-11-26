use std::time::Duration;

use anyhow::Result;
use bevy::prelude::World;
use tracing::{info, trace};

use crate::connection::events::IterMessageEvent;
use crate::connection::ProtocolMessage;
use crate::inputs::input_buffer::InputBuffer;
use crate::packet::packet_manager::Payload;
use crate::{
    ChannelKind, ChannelRegistry, ConnectionEvents, InputMessage, PingChannel, PingMessage,
    Protocol, SyncMessage, TickManager, TimeManager, TimeSyncPingMessage,
};

use super::ping_manager::{PingConfig, PingManager};

// TODO: this layer of indirection is annoying, is there a better way?
//  maybe just pass the inner connection to ping_manager? (but harder to test)
pub struct Connection<P: Protocol> {
    pub(crate) base: crate::Connection<P>,

    pub(crate) input_buffer: InputBuffer<P::Input>,
    pub(crate) ping_manager: PingManager,
}

impl<P: Protocol> Connection<P> {
    pub fn new(channel_registry: &ChannelRegistry, ping_config: &PingConfig) -> Self {
        Self {
            base: crate::Connection::new(channel_registry),
            input_buffer: InputBuffer::default(),
            ping_manager: PingManager::new(ping_config),
        }
    }

    /// Read messages received from buffer (either messages or replication events) and push them to events
    /// Also update the input buffer
    pub fn receive(
        &mut self,
        world: &mut World,
        time_manager: &TimeManager,
    ) -> ConnectionEvents<P> {
        let mut events = self.base.receive(world, time_manager);
        if events.has_messages::<InputMessage<P::Input>>() {
            trace!("update input buffer");
            // this has the added advantage that we remove the InputMessages so we don't read them later
            let input_messages: Vec<_> = events
                .into_iter_messages::<InputMessage<P::Input>>()
                .map(|(input_message, _)| input_message)
                .collect();
            for input_message in input_messages {
                // info!("Received input message: {:?}", input_message);
                self.input_buffer.update_from_message(input_message);
            }
        }
        events
    }

    pub fn update(
        &mut self,
        delta: Duration,
        time_manager: &TimeManager,
        tick_manager: &TickManager,
    ) {
        self.base.update(time_manager, tick_manager);
        // TODO: handle deltas inside time_manager
        self.ping_manager.update(delta);

        // maybe send pings
        self.maybe_buffer_ping(time_manager);
    }

    // TODO: rename to maybe send ping?
    pub fn maybe_buffer_ping(&mut self, time_manager: &TimeManager) -> Result<()> {
        if !self.ping_manager.should_send_ping() {
            return Ok(());
        }

        let ping_message = self.ping_manager.prepare_ping(time_manager);

        // info!("Sending ping {:?}", ping_message);
        trace!("Sending ping {:?}", ping_message);

        let message = ProtocolMessage::Sync(SyncMessage::Ping(ping_message));
        let channel = ChannelKind::of::<PingChannel>();
        self.base.message_manager.buffer_send(message, channel)
    }

    pub fn buffer_pong(&mut self, time_manager: &TimeManager, ping: PingMessage) -> Result<()> {
        let pong_message = self.ping_manager.prepare_pong(time_manager, ping);

        // info!("Sending ping {:?}", ping_message);
        trace!("Sending pong {:?}", pong_message);
        let message = ProtocolMessage::Sync(SyncMessage::Pong(pong_message));
        let channel = ChannelKind::of::<PingChannel>();
        self.base.message_manager.buffer_send(message, channel)
    }

    pub fn buffer_sync_pong(
        &mut self,
        time_manager: &TimeManager,
        tick_manager: &TickManager,
        ping: TimeSyncPingMessage,
    ) -> Result<()> {
        let pong_message = self
            .ping_manager
            .prepare_sync_pong(time_manager, tick_manager, ping);

        // info!("Sending ping {:?}", ping_message);
        trace!("Sending time sync pong {:?}", pong_message);
        let message = ProtocolMessage::Sync(SyncMessage::TimeSyncPong(pong_message));
        let channel = ChannelKind::of::<PingChannel>();
        self.base.message_manager.buffer_send(message, channel)
    }

    pub fn send_packets(
        &mut self,
        time_manager: &TimeManager,
        tick_manager: &TickManager,
    ) -> Result<Vec<Payload>> {
        if time_manager.is_ready_to_send() {
            // prepare the time-sync-pong messages with the correct send time
            self.ping_manager
                .client_pings_pending_pong()
                .into_iter()
                .try_for_each(|ping| self.buffer_sync_pong(time_manager, tick_manager, ping))?;
        }
        self.base.send_packets(tick_manager)
    }
}
