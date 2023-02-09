use tracing::warn;
use std::time::Duration;

use crate::shared::{
    serde::{BitReader, BitWriter, Serde},
    FakeEntityConverter, Message, Messages,
};
pub use crate::shared::{
    ConnectionConfig, PacketType, StandardHeader, Timer, Timestamp as stamp_time,
};

use super::io::Io;

pub type Timestamp = u64;

#[derive(Debug, Eq, PartialEq)]
pub enum HandshakeState {
    AwaitingChallengeResponse,
    AwaitingConnectResponse,
    Connected,
}

pub enum HandshakeResult {
    Connected,
    Rejected,
}

pub struct HandshakeManager {
    handshake_timer: Timer,
    pre_connection_timestamp: Timestamp,
    pre_connection_digest: Option<Vec<u8>>,
    pub connection_state: HandshakeState,
    auth_message: Option<Box<dyn Message>>,
}

impl HandshakeManager {
    pub fn new(send_interval: Duration) -> Self {
        let mut handshake_timer = Timer::new(send_interval);
        handshake_timer.ring_manual();

        let pre_connection_timestamp = stamp_time::now();

        Self {
            handshake_timer,
            pre_connection_timestamp,
            pre_connection_digest: None,
            connection_state: HandshakeState::AwaitingChallengeResponse,
            auth_message: None,
        }
    }

    pub fn set_auth_message(&mut self, auth: Box<dyn Message>) {
        self.auth_message = Some(auth);
    }

    pub fn is_connected(&self) -> bool {
        self.connection_state == HandshakeState::Connected
    }

    // Give handshake manager the opportunity to send out messages to the server
    pub fn send(&mut self, io: &mut Io) {
        if io.is_loaded() {
            if !self.handshake_timer.ringing() {
                return;
            }

            self.handshake_timer.reset();

            match self.connection_state {
                HandshakeState::Connected => {
                    // do nothing, not necessary
                }
                HandshakeState::AwaitingChallengeResponse => {
                    let mut writer = self.write_challenge_request();
                    match io.send_writer(&mut writer) {
                        Ok(()) => {}
                        Err(_) => {
                            // TODO: pass this on and handle above
                            warn!("Client Error: Cannot send challenge request packet to Server");
                        }
                    }
                }
                HandshakeState::AwaitingConnectResponse => {
                    let mut writer = self.write_connect_request();
                    match io.send_writer(&mut writer) {
                        Ok(()) => {}
                        Err(_) => {
                            // TODO: pass this on and handle above
                            warn!("Client Error: Cannot send connect request packet to Server");
                        }
                    }
                }
            }
        }
    }

    // Call this regularly so handshake manager can process incoming requests
    pub fn recv(&mut self, reader: &mut BitReader) -> Option<HandshakeResult> {
        let header_result = StandardHeader::de(reader);
        if header_result.is_err() {
            return None;
        }
        let header = header_result.unwrap();
        match header.packet_type {
            PacketType::ServerChallengeResponse => {
                self.recv_challenge_response(reader);
                None
            }
            PacketType::ServerConnectResponse => self.recv_connect_response(),
            PacketType::ServerRejectResponse => Some(HandshakeResult::Rejected),
            _ => None,
        }
    }

    // Step 1 of Handshake
    pub fn write_challenge_request(&self) -> BitWriter {
        let mut writer = BitWriter::new();
        StandardHeader::new(PacketType::ClientChallengeRequest, 0, 0, 0).ser(&mut writer);

        self.pre_connection_timestamp.ser(&mut writer);

        writer
    }

    // Step 2 of Handshake
    pub fn recv_challenge_response(&mut self, reader: &mut BitReader) {
        if self.connection_state == HandshakeState::AwaitingChallengeResponse {
            let timestamp_result = Timestamp::de(reader);
            if timestamp_result.is_err() {
                return;
            }
            let timestamp = timestamp_result.unwrap();

            if self.pre_connection_timestamp == timestamp {
                let digest_bytes_result = Vec::<u8>::de(reader);
                if digest_bytes_result.is_err() {
                    return;
                }
                let digest_bytes = digest_bytes_result.unwrap();
                self.pre_connection_digest = Some(digest_bytes);

                self.connection_state = HandshakeState::AwaitingConnectResponse;
            }
        }
    }

    // Step 3 of Handshake
    pub fn write_connect_request(&self) -> BitWriter {
        let mut writer = BitWriter::new();

        StandardHeader::new(PacketType::ClientConnectRequest, 0, 0, 0).ser(&mut writer);

        // write timestamp & digest into payload
        self.write_signed_timestamp(&mut writer);

        // write auth message if there is one
        if let Some(auth_message) = &self.auth_message {
            // write that we have auth
            true.ser(&mut writer);
            // write payload
            Messages::write(&mut writer, &FakeEntityConverter, auth_message);
        } else {
            // write that we do not have auth
            false.ser(&mut writer);
        }

        writer
    }

    // Step 4 of Handshake
    pub fn recv_connect_response(&mut self) -> Option<HandshakeResult> {
        let was_not_connected = self.connection_state != HandshakeState::Connected;

        self.connection_state = HandshakeState::Connected;

        match was_not_connected {
            true => Some(HandshakeResult::Connected),
            false => None,
        }
    }

    // Send 10 disconnect packets
    pub fn write_disconnect(&self) -> BitWriter {
        let mut writer = BitWriter::new();
        StandardHeader::new(PacketType::Disconnect, 0, 0, 0).ser(&mut writer);
        self.write_signed_timestamp(&mut writer);
        writer
    }

    // Private methods

    fn write_signed_timestamp(&self, writer: &mut BitWriter) {
        self.pre_connection_timestamp.ser(writer);
        let digest: &Vec<u8> = self.pre_connection_digest.as_ref().unwrap();
        digest.ser(writer);
    }
}
