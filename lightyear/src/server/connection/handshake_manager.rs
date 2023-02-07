use std::{collections::HashMap, net::SocketAddr};

use ring::{hmac, rand};

use crate::shared::Messages;
pub use crate::shared::{
    serde::{BitReader, BitWriter, Serde, SerdeErr},
    wrapping_diff, BaseConnection, ConnectionConfig, FakeEntityConverter, Instant, KeyGenerator,
    Message, PacketType, PropertyMutate, PropertyMutator, Replicate, ReplicateSafe, StandardHeader,
    Timer,
};

use crate::server::cache_map::CacheMap;

use super::connection::Connection;

pub type Timestamp = u64;

pub enum HandshakeResult {
    Invalid,
    Success(Option<Box<dyn Message>>),
}

pub struct HandshakeManager {
    connection_hash_key: hmac::Key,
    require_auth: bool,
    address_to_timestamp_map: HashMap<SocketAddr, Timestamp>,
    timestamp_digest_map: CacheMap<Timestamp, Vec<u8>>,
}

impl HandshakeManager {
    pub fn new(require_auth: bool) -> Self {
        let connection_hash_key =
            hmac::Key::generate(hmac::HMAC_SHA256, &rand::SystemRandom::new()).unwrap();

        Self {
            connection_hash_key,
            require_auth,
            address_to_timestamp_map: HashMap::new(),
            timestamp_digest_map: CacheMap::with_capacity(64),
        }
    }

    // Step 1 of Handshake
    pub fn recv_challenge_request(
        &mut self,
        reader: &mut BitReader,
    ) -> Result<BitWriter, SerdeErr> {
        let timestamp = Timestamp::de(reader)?;

        Ok(self.write_challenge_response(&timestamp))
    }

    // Step 2 of Handshake
    pub fn write_challenge_response(&mut self, timestamp: &Timestamp) -> BitWriter {
        let mut writer = BitWriter::new();
        StandardHeader::new(PacketType::ServerChallengeResponse, 0, 0, 0).ser(&mut writer);
        timestamp.ser(&mut writer);

        if !self.timestamp_digest_map.contains_key(timestamp) {
            let tag = hmac::sign(&self.connection_hash_key, &timestamp.to_le_bytes());
            let tag_vec: Vec<u8> = Vec::from(tag.as_ref());
            self.timestamp_digest_map.insert(*timestamp, tag_vec);
        }

        //write timestamp digest
        self.timestamp_digest_map
            .get_unchecked(timestamp)
            .ser(&mut writer);

        writer
    }

    // Step 3 of Handshake
    pub fn recv_connect_request(
        &mut self,
        address: &SocketAddr,
        reader: &mut BitReader,
    ) -> HandshakeResult {
        // Verify that timestamp hash has been written by this
        // server instance
        if let Some(timestamp) = self.timestamp_validate(reader) {
            // Timestamp hash is validated, now start configured auth process
            if let Ok(has_auth) = bool::de(reader) {
                if has_auth != self.require_auth {
                    return HandshakeResult::Invalid;
                }

                self.address_to_timestamp_map.insert(*address, timestamp);

                if has_auth {
                    if let Ok(auth_message) = Messages::read(reader, &FakeEntityConverter) {
                        HandshakeResult::Success(Some(auth_message))
                    } else {
                        HandshakeResult::Invalid
                    }
                } else {
                    HandshakeResult::Success(None)
                }
            } else {
                HandshakeResult::Invalid
            }
        } else {
            HandshakeResult::Invalid
        }
    }

    // Step 3 of Handshake
    pub fn write_connect_response(&self) -> BitWriter {
        let mut writer = BitWriter::new();
        StandardHeader::new(PacketType::ServerConnectResponse, 0, 0, 0).ser(&mut writer);
        writer
    }

    pub fn verify_disconnect_request(
        &mut self,
        connection: &Connection,
        reader: &mut BitReader,
    ) -> bool {
        // Verify that timestamp hash has been written by this
        // server instance
        if let Some(new_timestamp) = self.timestamp_validate(reader) {
            if let Some(old_timestamp) = self.address_to_timestamp_map.get(&connection.base.address)
            {
                if *old_timestamp == new_timestamp {
                    return true;
                }
            }
        }

        false
    }

    pub fn write_reject_response(&self) -> BitWriter {
        let mut writer = BitWriter::new();
        StandardHeader::new(PacketType::ServerRejectResponse, 0, 0, 0).ser(&mut writer);
        writer
    }

    pub fn delete_user(&mut self, address: &SocketAddr) {
        self.address_to_timestamp_map.remove(address);
    }

    fn timestamp_validate(&self, reader: &mut BitReader) -> Option<Timestamp> {
        // Read timestamp
        let timestamp_result = Timestamp::de(reader);
        if timestamp_result.is_err() {
            return None;
        }
        let timestamp = timestamp_result.unwrap();

        // Read digest
        let digest_bytes_result = Vec::<u8>::de(reader);
        if digest_bytes_result.is_err() {
            return None;
        }
        let digest_bytes = digest_bytes_result.unwrap();

        // Verify that timestamp hash has been written by this server instance
        let validation_result = hmac::verify(
            &self.connection_hash_key,
            &timestamp.to_le_bytes(),
            &digest_bytes,
        );
        if validation_result.is_err() {
            None
        } else {
            Some(timestamp)
        }
    }
}
