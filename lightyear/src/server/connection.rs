//! Wrapper around [`crate::connection::Connection`] that adds server-specific functionality
use std::pin::pin;
use std::time::Duration;

use crate::_reexport::ReadBuffer;
use anyhow::Result;
use bevy::ecs::component::Tick as BevyTick;
use bevy::prelude::World;
use tracing::trace;

use crate::channel::builder::PingChannel;
use crate::connection::events::{ConnectionEvents, IterMessageEvent};
use crate::connection::message::ProtocolMessage;
use crate::inputs::input_buffer::{InputBuffer, InputMessage};
use crate::packet::packet_manager::Payload;
use crate::protocol::channel::{ChannelKind, ChannelRegistry};
use crate::protocol::Protocol;
use crate::shared::ping::manager::{PingConfig, PingManager};
use crate::shared::ping::message::{Ping, Pong, SyncMessage};
use crate::shared::tick_manager::TickManager;
use crate::shared::time_manager::TimeManager;

/// Wrapper around a [`crate::connection::Connection`] with server-specific logic
/// (handling client inputs)
pub struct Connection<P: Protocol> {
    pub(crate) base: crate::connection::Connection<P>,
    /// Stores the inputs that we have received from the client.
    pub(crate) input_buffer: InputBuffer<P::Input>,
    /// Stores the last input we have received from the client.
    /// In case we are missing the client input for a tick, we will fallback to using this.
    pub(crate) last_input: Option<P::Input>,
}

impl<P: Protocol> Connection<P> {
    pub fn new(channel_registry: &ChannelRegistry, ping_config: &PingConfig) -> Self {
        Self {
            base: crate::connection::Connection::new(channel_registry, ping_config),
            input_buffer: InputBuffer::default(),
            last_input: None,
        }
    }

    /// Receive a packet and buffer it
    /// Also handle any acks that we read from that packet
    pub fn recv_packet(&mut self, reader: &mut impl ReadBuffer, bevy_tick: BevyTick) -> Result<()> {
        // receive packet
        self.base.recv_packet(reader)?;

        // TODO: maybe do this also on client connection? Instead of only on server connection?
        // update the bevy ticks associated with the update messages
        self.base.replication_manager.recv_update_acks(bevy_tick);

        Ok(())
    }

    /// Read messages received from buffer (either messages or replication events) and push them to events
    /// Also update the input buffer
    pub fn receive(
        &mut self,
        world: &mut World,
        time_manager: &TimeManager,
    ) -> ConnectionEvents<P> {
        let mut events = self.base.receive(world, time_manager);
        // inputs
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
}
