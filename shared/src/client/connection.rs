use std::time::Duration;

use anyhow::Result;

use crate::connection::ProtocolMessage;
use crate::inputs::input_buffer::InputBuffer;
use crate::packet::packet::PacketId;
use crate::packet::packet_manager::Payload;
use crate::tick::Tick;
use crate::{
    ChannelKind, ChannelRegistry, PingChannel, Protocol, ReadBuffer, SyncMessage, TickManager,
    TimeManager,
};

use super::ping_manager::PingConfig;
use super::sync::SyncManager;

// TODO: this layer of indirection is annoying, is there a better way?
//  maybe just pass the inner connection to ping_manager? (but harder to test)
pub struct Connection<P: Protocol> {
    pub(crate) base: crate::Connection<P>,

    // pub(crate) ping_manager: PingManager,
    pub(crate) input_buffer: InputBuffer<P::Input>,
    pub(crate) sync_manager: SyncManager,
    // TODO: maybe don't do any replication until connection is synced?
}

impl<P: Protocol> Connection<P> {
    pub fn new(channel_registry: &ChannelRegistry, ping_config: &PingConfig) -> Self {
        Self {
            base: crate::Connection::new(channel_registry),
            // ping_manager: PingManager::new(ping_config),
            input_buffer: InputBuffer::default(),
            sync_manager: SyncManager::new(
                ping_config.sync_num_pings,
                ping_config.sync_ping_interval_ms,
            ),
        }
    }

    /// Add an input for the given tick
    pub fn add_input(&mut self, input: P::Input, tick: Tick) {
        self.input_buffer.buffer.push(&tick, input);
    }

    pub fn update(&mut self, time_manager: &TimeManager, tick_manager: &TickManager) {
        self.base.update(time_manager, tick_manager);
        self.sync_manager.update(time_manager);
        // TODO: maybe prepare ping?
        // self.ping_manager.update(delta);

        // client send pings to estimate rtt
        if let Some(sync_ping) = self
            .sync_manager
            .maybe_prepare_ping(time_manager, tick_manager)
        {
            let message = ProtocolMessage::Sync(SyncMessage::TimeSyncPing(sync_ping));
            let channel = ChannelKind::of::<PingChannel>();
            self.base
                .message_manager
                .buffer_send(message, channel)
                .unwrap();
        }
    }

    pub fn send_packets(
        &mut self,
        time_manager: &TimeManager,
        tick_manager: &TickManager,
    ) -> Result<Vec<(Payload, PacketId)>> {
        Ok(self
            .base
            .message_manager
            .send_packets(tick_manager.current_tick())?
            .iter()
            .map(|(payload, packet_id)| {
                // record the packet send time in the sync manager (so that when we receive an ack for that packet
                // we can estimate the RTT)
                // TODO: the problem with this approach is that the server might not be (or even running receive) sending packets back every frame!
                //  potential delays not accounted for:
                //  - the server received the packet physically but didn't call io.recv()
                //  - the packet was io.recv() but the server doesn't send any packet back
                //  - time between recv() and send()
                //  THIS MEANS WE SHOULD ONLY SEND PING MESSAGES FOR RTT! ALSO THE PONG INCLUDES
                //  SERVER_RECV_TIME AND SERVER_SEND_TIME, SO WE CAN REMOVE THAT TIME FROM THE RTT

                //  if it doesn't, then the ack time cannot be used for RTT.
                //  But realistic the server does
                self.sync_manager
                    .record_sent_packet(*packet_id, time_manager.now());

                payload
            })
            .collect())
    }

    pub fn recv_packet(&mut self, reader: &mut impl ReadBuffer) -> Result<()> {
        let tick = self.base.recv_packet(reader)?;
        if tick > self.sync_manager.latest_received_server_tick {
            self.sync_manager.latest_received_server_tick = tick;
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
