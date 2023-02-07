use tracing::warn;
use std::net::SocketAddr;
use bevy_ecs::world::World;

use crate::shared::{
    sequence_greater_than,
    serde::{BitReader, BitWriter, Serde, SerdeErr},
    BaseConnection, ConnectionConfig, HostType, Instant, PacketType, PingManager,
    ProtocolIo, StandardHeader, Tick,
};

use crate::server::{
    protocol::entity_manager::EntityManager,
    tick::{tick_buffer_receiver::TickBufferReceiver, tick_manager::TickManager},
    user::UserKey,
};

use super::io::Io;

pub struct Connection {
    pub user_key: UserKey,
    pub base: BaseConnection,
    pub entity_manager: EntityManager,
    pub tick_buffer: TickBufferReceiver,
    pub last_received_tick: Tick,
    pub ping_manager: PingManager,
}

impl Connection {
    pub fn new(
        connection_config: &ConnectionConfig,
        user_address: SocketAddr,
        user_key: &UserKey,
    ) -> Self {
        Connection {
            user_key: *user_key,
            base: BaseConnection::new(user_address, HostType::Server, connection_config),
            entity_manager: EntityManager::new(user_address),
            tick_buffer: TickBufferReceiver::new(),
            ping_manager: PingManager::new(&connection_config.ping),
            last_received_tick: 0,
        }
    }

    // Incoming Data

    pub fn process_incoming_header(&mut self, header: &StandardHeader) {
        self.base
            .process_incoming_header(header, &mut Some(&mut self.entity_manager));
    }

    /// Update the last received tick tracker from the given client
    pub fn recv_client_tick(&mut self, client_tick: Tick) {
        if sequence_greater_than(client_tick, self.last_received_tick) {
            self.last_received_tick = client_tick;
        }
    }

    /// Read packet data received from a client
    pub fn process_incoming_data(
        &mut self,
        server_and_client_tick_opt: Option<(Tick, Tick)>,
        reader: &mut BitReader,
    ) -> Result<(), SerdeErr> {
        let converter = &self.entity_manager;
        let channel_reader = ProtocolIo::new(converter);

        // read tick-buffered messages
        {
            if let Some((server_tick, client_tick)) = server_and_client_tick_opt {
                self.tick_buffer.read_messages(
                    &server_tick,
                    &client_tick,
                    &channel_reader,
                    reader,
                )?;
            }
        }

        // read messages
        {
            self.base
                .message_manager
                .read_messages(&channel_reader, reader)?;
        }

        Ok(())
    }

    // Outgoing data
    pub fn send_outgoing_packets(
        &mut self,
        now: &Instant,
        io: &mut Io,
        world: &World,
        tick_manager_opt: &Option<TickManager>,
        rtt_millis: &f32,
    ) {
        self.collect_outgoing_messages(now, rtt_millis);

        let mut any_sent = false;
        loop {
            if self.send_outgoing_packet(now, io, world, tick_manager_opt) {
                any_sent = true;
            } else {
                break;
            }
        }
        if any_sent {
            self.base.mark_sent();
        }
    }

    fn collect_outgoing_messages(&mut self, now: &Instant, rtt_millis: &f32) {
        self.entity_manager.collect_outgoing_messages(
            now,
            rtt_millis,
            &mut self.base.message_manager,
        );
        self.base
            .message_manager
            .collect_outgoing_messages(now, rtt_millis);
    }

    /// Send any message, component actions and component updates to the client
    /// Will split the data into multiple packets.
    fn send_outgoing_packet(
        &mut self,
        now: &Instant,
        io: &mut Io,
        world: &World,
        tick_manager_opt: &Option<TickManager>,
    ) -> bool {
        // Check if we have messages to write. (Some channels could still want to write messages because of failed delivery)
        if self.base.message_manager.has_outgoing_messages() || self.entity_manager.has_outgoing_messages() {
            let next_packet_index = self.base.next_packet_index();

            let mut bit_writer = BitWriter::new();

            // Reserve bits we know will be required to finish the message:
            // 1. Messages finish bit
            // 2. Updates finish bit
            // 3. Actions finish bit
            bit_writer.reserve_bits(3);

            // write header
            self.base.write_outgoing_header(PacketType::Data, &mut bit_writer);

            // write server tick
            if let Some(tick_manager) = tick_manager_opt {
                tick_manager.write_server_tick(&mut bit_writer);
            }

            // info!("-- packet: {} --", next_packet_index);
            // if self.base.message_manager.has_outgoing_messages() {
            //     info!("writing some messages");
            // }

            let mut has_written = false;

            // write messages
            {
                let converter = &self.entity_manager;
                let channel_writer = ProtocolIo::new(converter);
                self.base.message_manager.write_messages(
                    &channel_writer,
                    &mut bit_writer,
                    next_packet_index,
                    &mut has_written,
                );

                // finish messages
                false.ser(&mut bit_writer);
                bit_writer.release_bits(1);
            }

            // write entity updates
            {
                self.entity_manager.write_updates(
                    now,
                    &mut bit_writer,
                    &next_packet_index,
                    world,
                    &mut has_written,
                );

                // finish updates
                false.ser(&mut bit_writer);
                bit_writer.release_bits(1);
            }

            // write entity actions
            {
                self.entity_manager.write_actions(
                    now,
                    &mut bit_writer,
                    &next_packet_index,
                    world,
                    &mut has_written,
                );

                // finish actions
                false.ser(&mut bit_writer);
                bit_writer.release_bits(1);
            }

            //info!("--------------\n");

            // send packet
            match io.send_writer(&self.base.address, &mut bit_writer) {
                Ok(()) => {}
                Err(_) => {
                    // TODO: pass this on and handle above
                    warn!(
                        "Server Error: Cannot send data packet to {}",
                        &self.base.address
                    );
                }
            }

            return true;
        }

        false
    }
}
