//! Wrapper around [`crate::connection::Connection`] that adds client-specific functionality

use std::time::Duration;

use anyhow::Result;
use bevy::prelude::World;
use tracing::{debug, info, trace};

use crate::channel::builder::PingChannel;
use crate::client::sync::SyncConfig;
use crate::connection::events::ConnectionEvents;
use crate::connection::message::ProtocolMessage;
use crate::inputs::input_buffer::InputBuffer;
use crate::packet::packet_manager::Payload;
use crate::protocol::channel::{ChannelKind, ChannelRegistry};
use crate::protocol::Protocol;
use crate::serialize::reader::ReadBuffer;
use crate::shared::ping::manager::{PingConfig, PingManager};
use crate::shared::ping::message::SyncMessage;
use crate::shared::tick_manager::Tick;
use crate::shared::tick_manager::TickManager;
use crate::shared::time_manager::TimeManager;

use super::sync::SyncManager;

/// Wrapper around a [`crate::connection::Connection`] with client-specific logic
/// (handling player inputs, and syncing the time between client and server)
pub struct Connection<P: Protocol> {
    pub(crate) base: crate::connection::Connection<P>,

    pub(crate) input_buffer: InputBuffer<P::Input>,
    pub(crate) sync_manager: SyncManager,
    // TODO: maybe don't do any replication until connection is synced?
}

impl<P: Protocol> Connection<P> {
    pub fn new(
        channel_registry: &ChannelRegistry,
        sync_config: SyncConfig,
        ping_config: &PingConfig,
    ) -> Self {
        Self {
            base: crate::connection::Connection::new(channel_registry, ping_config),
            input_buffer: InputBuffer::default(),
            sync_manager: SyncManager::new(sync_config),
        }
    }

    /// Add an input for the given tick
    pub fn add_input(&mut self, input: P::Input, tick: Tick) {
        self.input_buffer.set(tick, Some(input));
    }

    pub fn update(&mut self, time_manager: &TimeManager, tick_manager: &TickManager) {
        self.base.update(time_manager, tick_manager);
        // self.sync_manager.update(time_manager);
    }

    pub fn recv_packet(
        &mut self,
        reader: &mut impl ReadBuffer,
        time_manager: &TimeManager,
        tick_manager: &TickManager,
    ) -> Result<()> {
        let tick = self.base.recv_packet(reader)?;
        debug!("Received server packet with tick: {:?}", tick);
        if tick >= self.sync_manager.latest_received_server_tick {
            self.sync_manager.latest_received_server_tick = tick;
            // TODO: add 'received_new_server_tick' ?
            // we probably actually physically received the packet some time between our last `receive` and now.
            // Let's add delta / 2 as a compromise
            self.sync_manager.duration_since_latest_received_server_tick = Duration::default();
            // self.sync_manager.duration_since_latest_received_server_tick = time_manager.delta() / 2;
            self.sync_manager.update_server_time_estimate(
                tick_manager.config.tick_duration,
                self.base.ping_manager.rtt(),
            );
        }
        debug!(?tick, last_server_tick = ?self.sync_manager.latest_received_server_tick, "Recv server packet");
        Ok(())
    }

    #[cfg(test)]
    pub fn base(&self) -> &crate::connection::Connection<P> {
        &self.base
    }
}
