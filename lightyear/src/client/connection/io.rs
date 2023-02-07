use std::{net::SocketAddr, time::Duration};

use naia_client_socket::{NaiaClientSocketError, PacketReceiver, PacketSender, ServerAddr};

pub use crate::shared::{
    serde::{BitReader, BitWriter},
    BandwidthMonitor, CompressionConfig, ConnectionConfig, Decoder, Encoder, PacketType,
    ReplicateSafe, StandardHeader, Timer, Timestamp,
};

use crate::client::error::NaiaClientError;

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
                .client_to_server
                .as_ref()
                .map(|mode| Encoder::new(mode.clone()))
        });
        let incoming_decoder = compression_config.as_ref().and_then(|config| {
            config
                .server_to_client
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

    pub fn send_writer(&mut self, writer: &mut BitWriter) -> Result<(), NaiaClientSocketError> {
        // get payload
        let (length, buffer) = writer.flush();
        let mut payload = &buffer[0..length];

        // Compression
        if let Some(encoder) = &mut self.outgoing_encoder {
            payload = encoder.encode(payload);
        }

        // Bandwidth monitoring
        if let Some(monitor) = &mut self.outgoing_bandwidth_monitor {
            monitor.record_packet(payload.len());
        }

        self.packet_sender
            .as_mut()
            .expect("Cannot call Client.send_packet() until you call Client.connect()!")
            .send(payload)
            .map_err(|_| NaiaClientSocketError::SendError)
    }

    pub fn recv_reader(&mut self) -> Result<Option<BitReader>, NaiaClientSocketError> {
        let receive_result = self
            .packet_receiver
            .as_mut()
            .expect("Cannot call Client.receive_packet() until you call Client.connect()!")
            .receive();

        if let Ok(Some(mut payload)) = receive_result {
            // Bandwidth monitoring
            if let Some(monitor) = &mut self.incoming_bandwidth_monitor {
                monitor.record_packet(payload.len());
            }

            // Decompression
            if let Some(decoder) = &mut self.incoming_decoder {
                payload = decoder.decode(payload);
            }

            Ok(Some(BitReader::new(payload)))
        } else {
            receive_result.map(|payload_opt| payload_opt.map(BitReader::new))
        }
    }

    pub fn server_addr(&self) -> Result<SocketAddr, NaiaClientError> {
        if let Some(packet_sender) = self.packet_sender.as_ref() {
            if let ServerAddr::Found(server_addr) = packet_sender.server_addr() {
                Ok(server_addr)
            } else {
                Err(NaiaClientError::from_message("Connection has not yet been established! Make sure you call Client.connect() before calling this."))
            }
        } else {
            Err(NaiaClientError::from_message("Connection has not yet been established! Make sure you call Client.connect() before calling this."))
        }
    }

    pub fn outgoing_bandwidth(&mut self) -> f32 {
        return self
            .outgoing_bandwidth_monitor
            .as_mut()
            .expect("Need to call `enable_bandwidth_monitor()` on Io before calling this")
            .bandwidth();
    }

    pub fn incoming_bandwidth(&mut self) -> f32 {
        return self
            .incoming_bandwidth_monitor
            .as_mut()
            .expect("Need to call `enable_bandwidth_monitor()` on Io before calling this")
            .bandwidth();
    }
}
