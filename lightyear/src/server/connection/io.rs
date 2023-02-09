use std::{net::SocketAddr, panic, time::Duration};

use naia_server_socket::{NaiaServerSocketError, PacketReceiver, PacketSender};

pub use crate::shared::{
    serde::{BitWriter, OwnedBitReader},
    wrapping_diff, BaseConnection, CompressionConfig, ConnectionConfig, Decoder, Encoder, Instant,
    KeyGenerator, PacketType,
    StandardHeader, Timer, Timestamp,
};

use super::bandwidth_monitor::BandwidthMonitor;

pub struct Io {
    packet_sender: Option<PacketSender>,
    packet_receiver: Option<PacketReceiver>,
    outgoing_bandwidth_monitor: Option<BandwidthMonitor>,
    incoming_bandwidth_monitor: Option<BandwidthMonitor>,
    outgoing_encoder: Option<Encoder>,
    incoming_decoder: Option<Decoder>,
}

impl Io {
    pub fn new(
        bandwidth_measure_duration: &Option<Duration>,
        compression_config: &Option<CompressionConfig>,
    ) -> Self {
        let outgoing_bandwidth_monitor = bandwidth_measure_duration.map(BandwidthMonitor::new);
        let incoming_bandwidth_monitor = bandwidth_measure_duration.map(BandwidthMonitor::new);

        let outgoing_encoder = compression_config.as_ref().and_then(|config| {
            config
                .server_to_client
                .as_ref()
                .map(|mode| Encoder::new(mode.clone()))
        });
        let incoming_decoder = compression_config.as_ref().and_then(|config| {
            config
                .client_to_server
                .as_ref()
                .map(|mode| Decoder::new(mode.clone()))
        });

        Io {
            packet_sender: None,
            packet_receiver: None,
            outgoing_bandwidth_monitor,
            incoming_bandwidth_monitor,
            outgoing_encoder,
            incoming_decoder,
        }
    }

    pub fn load(&mut self, packet_sender: PacketSender, packet_receiver: PacketReceiver) {
        if self.packet_sender.is_some() {
            panic!("Packet sender/receiver already loaded! Cannot do this twice!");
        }

        self.packet_sender = Some(packet_sender);
        self.packet_receiver = Some(packet_receiver);
    }

    pub fn is_loaded(&self) -> bool {
        self.packet_sender.is_some()
    }

    pub fn send_writer(
        &mut self,
        address: &SocketAddr,
        writer: &mut BitWriter,
    ) -> Result<(), NaiaServerSocketError> {
        // get payload
        let (length, buffer) = writer.flush();
        let mut payload = &buffer[0..length];

        // Compression
        if let Some(encoder) = &mut self.outgoing_encoder {
            payload = encoder.encode(payload);
        }

        // Bandwidth monitoring
        if let Some(monitor) = &mut self.outgoing_bandwidth_monitor {
            monitor.record_packet(address, payload.len());
        }

        self.packet_sender
            .as_ref()
            .expect("Cannot call Server.send_packet() until you call Server.listen()!")
            .send(address, payload)
            .map_err(|_| NaiaServerSocketError::SendError(*address))
    }

    pub fn recv_reader(
        &mut self,
    ) -> Result<Option<(SocketAddr, OwnedBitReader)>, NaiaServerSocketError> {
        let receive_result = self
            .packet_receiver
            .as_mut()
            .expect("Cannot call Server.receive_packet() until you call Server.listen()!")
            .receive();

        match receive_result {
            Ok(Some((address, mut payload))) => {
                // Bandwidth monitoring
                if let Some(monitor) = &mut self.incoming_bandwidth_monitor {
                    monitor.record_packet(&address, payload.len());
                }

                // Decompression
                if let Some(decoder) = &mut self.incoming_decoder {
                    payload = decoder.decode(payload);
                }

                Ok(Some((address, OwnedBitReader::new(payload))))
            }
            Ok(None) => Ok(None),
            Err(err) => Err(err),
        }
    }

    pub fn bandwidth_monitor_enabled(&self) -> bool {
        self.outgoing_bandwidth_monitor.is_some() && self.incoming_bandwidth_monitor.is_some()
    }

    pub fn register_client(&mut self, address: &SocketAddr) {
        self.outgoing_bandwidth_monitor
            .as_mut()
            .expect("Need to call `enable_bandwidth_monitor()` on Io before calling this")
            .create_client(address);
        self.incoming_bandwidth_monitor
            .as_mut()
            .expect("Need to call `enable_bandwidth_monitor()` on Io before calling this")
            .create_client(address);
    }

    pub fn deregister_client(&mut self, address: &SocketAddr) {
        self.outgoing_bandwidth_monitor
            .as_mut()
            .expect("Need to call `enable_bandwidth_monitor()` on Io before calling this")
            .delete_client(address);
        self.incoming_bandwidth_monitor
            .as_mut()
            .expect("Need to call `enable_bandwidth_monitor()` on Io before calling this")
            .delete_client(address);
    }

    pub fn outgoing_bandwidth_total(&mut self) -> f32 {
        return self
            .outgoing_bandwidth_monitor
            .as_mut()
            .expect("Need to call `enable_bandwidth_monitor()` on Io before calling this")
            .total_bandwidth();
    }

    pub fn incoming_bandwidth_total(&mut self) -> f32 {
        return self
            .incoming_bandwidth_monitor
            .as_mut()
            .expect("Need to call `enable_bandwidth_monitor()` on Io before calling this")
            .total_bandwidth();
    }

    pub fn outgoing_bandwidth_to_client(&mut self, address: &SocketAddr) -> f32 {
        return self
            .outgoing_bandwidth_monitor
            .as_mut()
            .expect("Need to call `enable_bandwidth_monitor()` on Io before calling this")
            .client_bandwidth(address);
    }

    pub fn incoming_bandwidth_from_client(&mut self, address: &SocketAddr) -> f32 {
        return self
            .incoming_bandwidth_monitor
            .as_mut()
            .expect("Need to call `enable_bandwidth_monitor()` on Io before calling this")
            .client_bandwidth(address);
    }
}
