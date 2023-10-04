use std::io::{self, Cursor, Write};

use super::crypto::{dencrypted_in_place, encrypt_in_place};
use super::replay_protection::ReplayProtection;
use super::token::ConnectToken;
use super::{constants::*, error::NetcodeError, serialize::*};

#[derive(Debug)]
#[repr(u8)]
pub enum PacketType {
    ConnectionRequest = 0,
    ConnectionDenied = 1,
    Challenge = 2,
    Response = 3,
    KeepAlive = 4,
    Payload = 5,
    Disconnect = 6,
}

#[derive(Debug, PartialEq, Eq)]
#[allow(clippy::large_enum_variant)] // TODO: Consider boxing types
pub enum Packet<'a> {
    ConnectionRequest {
        version_info: [u8; 13], // "NETCODE 1.02" ASCII with null terminator.
        protocol_id: u64,
        expire_timestamp: u64,
        xnonce: [u8; NETCODE_CONNECT_TOKEN_XNONCE_BYTES],
        data: [u8; NETCODE_CONNECT_TOKEN_PRIVATE_BYTES],
    },
    ConnectionDenied,
    Challenge {
        token_sequence: u64,
        token_data: [u8; NETCODE_CHALLENGE_TOKEN_BYTES], // encrypted ChallengeToken
    },
    Response {
        token_sequence: u64,
        token_data: [u8; NETCODE_CHALLENGE_TOKEN_BYTES], // encrypted ChallengeToken
    },
    KeepAlive {
        client_index: u32,
        max_clients: u32,
    },
    Payload(&'a [u8]),
    Disconnect,
}

#[derive(Debug, PartialEq, Eq)]
pub struct ChallengeToken {
    pub client_id: u64,
    pub user_data: [u8; 256],
}

impl PacketType {
    fn from_u8(value: u8) -> Result<Self, NetcodeError> {
        use PacketType::*;

        let packet_type = match value {
            0 => ConnectionRequest,
            1 => ConnectionDenied,
            2 => Challenge,
            3 => Response,
            4 => KeepAlive,
            5 => Payload,
            6 => Disconnect,
            _ => return Err(NetcodeError::InvalidPacketType),
        };
        Ok(packet_type)
    }

    fn apply_replay_protection(&self) -> bool {
        use PacketType::*;

        matches!(self, KeepAlive | Payload | Disconnect)
    }
}

impl<'a> Packet<'a> {
    pub fn packet_type(&self) -> PacketType {
        match self {
            Packet::ConnectionRequest { .. } => PacketType::ConnectionRequest,
            Packet::ConnectionDenied => PacketType::ConnectionDenied,
            Packet::Challenge { .. } => PacketType::Challenge,
            Packet::Response { .. } => PacketType::Response,
            Packet::KeepAlive { .. } => PacketType::KeepAlive,
            Packet::Payload { .. } => PacketType::Payload,
            Packet::Disconnect => PacketType::Disconnect,
        }
    }

    pub fn id(&self) -> u8 {
        self.packet_type() as u8
    }

    pub fn connection_request_from_token(connect_token: &ConnectToken) -> Self {
        Packet::ConnectionRequest {
            xnonce: connect_token.xnonce,
            version_info: *NETCODE_VERSION_INFO,
            protocol_id: connect_token.protocol_id,
            expire_timestamp: connect_token.expire_timestamp,
            data: connect_token.private_data,
        }
    }

    pub fn generate_challenge(
        client_id: u64,
        user_data: &[u8; NETCODE_USER_DATA_BYTES],
        challenge_sequence: u64,
        challenge_key: &[u8; NETCODE_KEY_BYTES],
    ) -> Result<Self, NetcodeError> {
        let token = ChallengeToken::new(client_id, user_data);
        let mut buffer = [0u8; NETCODE_CHALLENGE_TOKEN_BYTES];
        token.write(&mut Cursor::new(&mut buffer[..]))?;
        encrypt_in_place(&mut buffer, challenge_sequence, challenge_key, b"")?;

        Ok(Packet::Challenge {
            token_sequence: challenge_sequence,
            token_data: buffer,
        })
    }

    fn write(&self, writer: &mut impl io::Write) -> Result<(), io::Error> {
        match self {
            Packet::ConnectionRequest {
                version_info,
                protocol_id,
                expire_timestamp,
                xnonce,
                data,
            } => {
                writer.write_all(version_info)?;
                writer.write_all(&protocol_id.to_le_bytes())?;
                writer.write_all(&expire_timestamp.to_le_bytes())?;
                writer.write_all(xnonce)?;
                writer.write_all(data)?;
            }
            Packet::Challenge {
                token_data,
                token_sequence,
            }
            | Packet::Response {
                token_data,
                token_sequence,
            } => {
                writer.write_all(&token_sequence.to_le_bytes())?;
                writer.write_all(token_data)?;
            }
            Packet::KeepAlive {
                max_clients,
                client_index,
            } => {
                writer.write_all(&client_index.to_le_bytes())?;
                writer.write_all(&max_clients.to_le_bytes())?;
            }
            Packet::Payload(p) => {
                writer.write_all(p)?;
            }
            Packet::ConnectionDenied | Packet::Disconnect => {}
        }

        Ok(())
    }

    fn read(packet_type: PacketType, src: &'a [u8]) -> Result<Self, io::Error> {
        if matches!(packet_type, PacketType::Payload) {
            return Ok(Packet::Payload(src));
        }

        let src = &mut Cursor::new(src);

        match packet_type {
            PacketType::ConnectionRequest => {
                let version_info = read_bytes(src)?;
                let protocol_id = read_u64(src)?;
                let expire_timestamp = read_u64(src)?;
                let xnonce = read_bytes(src)?;
                let token_data = read_bytes(src)?;

                Ok(Packet::ConnectionRequest {
                    version_info,
                    protocol_id,
                    expire_timestamp,
                    xnonce,
                    data: token_data,
                })
            }
            PacketType::Challenge => {
                let token_sequence = read_u64(src)?;
                let token_data = read_bytes(src)?;

                Ok(Packet::Challenge {
                    token_data,
                    token_sequence,
                })
            }
            PacketType::Response => {
                let token_sequence = read_u64(src)?;
                let token_data = read_bytes(src)?;

                Ok(Packet::Response {
                    token_data,
                    token_sequence,
                })
            }
            PacketType::KeepAlive => {
                let client_index = read_u32(src)?;
                let max_clients = read_u32(src)?;

                Ok(Packet::KeepAlive {
                    client_index,
                    max_clients,
                })
            }
            PacketType::ConnectionDenied => Ok(Packet::ConnectionDenied),
            PacketType::Disconnect => Ok(Packet::Disconnect),
            PacketType::Payload => unreachable!(),
        }
    }

    pub fn encode(
        &self,
        buffer: &mut [u8],
        protocol_id: u64,
        crypto_info: Option<(u64, &[u8; 32])>,
    ) -> Result<usize, NetcodeError> {
        if matches!(self, Packet::ConnectionRequest { .. }) {
            let mut writer = io::Cursor::new(buffer);
            let prefix_byte = encode_prefix(self.id(), 0);
            writer.write_all(&prefix_byte.to_le_bytes())?;

            self.write(&mut writer)?;
            Ok(writer.position() as usize)
        } else if let Some((sequence, private_key)) = crypto_info {
            let (start, end, aad) = {
                let mut writer = io::Cursor::new(&mut *buffer);
                let prefix_byte = {
                    let prefix_byte = encode_prefix(self.id(), sequence);
                    writer.write_all(&prefix_byte.to_le_bytes())?;
                    write_sequence(&mut writer, sequence)?;
                    prefix_byte
                };

                let start = writer.position() as usize;
                self.write(&mut writer)?;

                let additional_data = get_additional_data(prefix_byte, protocol_id);
                (start, writer.position() as usize, additional_data)
            };
            if buffer.len() < end + NETCODE_MAC_BYTES {
                return Err(NetcodeError::IoError(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "buffer too small to encode with encryption tag",
                )));
            }

            encrypt_in_place(
                &mut buffer[start..end + NETCODE_MAC_BYTES],
                sequence,
                private_key,
                &aad,
            )?;
            Ok(end + NETCODE_MAC_BYTES)
        } else {
            Err(NetcodeError::UnavailablePrivateKey)
        }
    }

    pub fn decode(
        mut buffer: &'a mut [u8],
        protocol_id: u64,
        private_key: Option<&[u8; 32]>,
        replay_protection: Option<&mut ReplayProtection>,
    ) -> Result<(u64, Self), NetcodeError> {
        if buffer.len() < 2 + NETCODE_MAC_BYTES {
            return Err(NetcodeError::PacketTooSmall);
        }

        let prefix_byte = buffer[0];
        let (packet_type, sequence_len) = decode_prefix(prefix_byte);
        let packet_type = PacketType::from_u8(packet_type)?;

        if matches!(packet_type, PacketType::ConnectionRequest) {
            Ok((
                0,
                Packet::read(PacketType::ConnectionRequest, &buffer[1..])?,
            ))
        } else if let Some(private_key) = private_key {
            let (sequence, aad, read_pos) = {
                let src = &mut io::Cursor::new(&mut buffer);
                src.set_position(1);
                let sequence = read_sequence(src, sequence_len)?;
                let additional_data = get_additional_data(prefix_byte, protocol_id);
                (sequence, additional_data, src.position() as usize)
            };

            if let Some(ref replay_protection) = replay_protection {
                if packet_type.apply_replay_protection()
                    && replay_protection.already_received(sequence)
                {
                    return Err(NetcodeError::DuplicatedSequence);
                }
            }

            dencrypted_in_place(&mut buffer[read_pos..], sequence, private_key, &aad)?;

            if let Some(replay_protection) = replay_protection {
                if packet_type.apply_replay_protection() {
                    replay_protection.advance_sequence(sequence);
                }
            }

            let packet = Packet::read(
                packet_type,
                &buffer[read_pos..buffer.len() - NETCODE_MAC_BYTES],
            )?;
            Ok((sequence, packet))
        } else {
            Err(NetcodeError::UnavailablePrivateKey)
        }
    }
}

impl ChallengeToken {
    pub fn new(client_id: u64, user_data: &[u8; NETCODE_USER_DATA_BYTES]) -> Self {
        Self {
            client_id,
            user_data: *user_data,
        }
    }

    fn read(src: &mut impl io::Read) -> Result<Self, io::Error> {
        let client_id = read_u64(src)?;
        let user_data: [u8; NETCODE_USER_DATA_BYTES] = read_bytes(src)?;

        Ok(Self {
            client_id,
            user_data,
        })
    }

    fn write(&self, out: &mut impl io::Write) -> Result<(), io::Error> {
        out.write_all(&self.client_id.to_le_bytes())?;
        out.write_all(&self.user_data)?;

        Ok(())
    }

    pub fn decode(
        token_data: [u8; NETCODE_CHALLENGE_TOKEN_BYTES],
        token_sequence: u64,
        challenge_key: &[u8; NETCODE_KEY_BYTES],
    ) -> Result<ChallengeToken, NetcodeError> {
        let mut decoded = [0u8; NETCODE_CHALLENGE_TOKEN_BYTES];
        decoded.copy_from_slice(&token_data);
        dencrypted_in_place(&mut decoded, token_sequence, challenge_key, b"")?;

        Ok(ChallengeToken::read(&mut Cursor::new(&mut decoded))?)
    }
}

fn get_additional_data(prefix: u8, protocol_id: u64) -> [u8; 13 + 8 + 1] {
    let mut buffer = [0; 13 + 8 + 1];
    buffer[..13].copy_from_slice(NETCODE_VERSION_INFO);
    buffer[13..21].copy_from_slice(&protocol_id.to_le_bytes());
    buffer[21] = prefix;

    buffer
}

fn decode_prefix(value: u8) -> (u8, usize) {
    ((value & 0xF), (value >> 4) as usize)
}

fn encode_prefix(value: u8, sequence: u64) -> u8 {
    value | ((sequence_bytes_required(sequence) as u8) << 4)
}

fn sequence_bytes_required(sequence: u64) -> usize {
    let mut mask: u64 = 0xFF00_0000_0000_0000;
    for i in 0..8 {
        if (sequence & mask) != 0x00 {
            return 8 - i;
        }

        mask >>= 8;
    }

    0
}

fn write_sequence(out: &mut impl io::Write, seq: u64) -> Result<usize, io::Error> {
    let len = sequence_bytes_required(seq);
    let sequence_scratch = seq.to_le_bytes();
    out.write(&sequence_scratch[..len])
}

fn read_sequence(source: &mut impl io::Read, len: usize) -> Result<u64, io::Error> {
    let mut seq_scratch = [0; 8];
    source.read_exact(&mut seq_scratch[0..len])?;
    Ok(u64::from_le_bytes(seq_scratch))
}

#[cfg(test)]
mod tests {
    use crate::netcode::{constants::*, crypto::generate_random_bytes};

    use super::*;

    #[test]
    fn connection_request_serialization() {
        let connection_request = Packet::ConnectionRequest {
            xnonce: generate_random_bytes(),
            version_info: [0; 13], // "NETCODE 1.02" ASCII with null terminator.
            protocol_id: 1,
            expire_timestamp: 3,
            data: [5; 1024],
        };
        let mut buffer = Vec::new();
        connection_request.write(&mut buffer).unwrap();
        let deserialized = Packet::read(PacketType::ConnectionRequest, &buffer).unwrap();

        assert_eq!(deserialized, connection_request);
    }

    #[test]
    fn connection_challenge_serialization() {
        let connection_challenge = Packet::Challenge {
            token_sequence: 0,
            token_data: [1u8; 300],
        };

        let mut buffer = Vec::new();
        connection_challenge.write(&mut buffer).unwrap();
        let deserialized = Packet::read(PacketType::Challenge, buffer.as_slice()).unwrap();

        assert_eq!(deserialized, connection_challenge);
    }

    #[test]
    fn connection_keep_alive_serialization() {
        let connection_keep_alive = Packet::KeepAlive {
            max_clients: 2,
            client_index: 1,
        };

        let mut buffer = Vec::new();
        connection_keep_alive.write(&mut buffer).unwrap();
        let deserialized = Packet::read(PacketType::KeepAlive, buffer.as_slice()).unwrap();

        assert_eq!(deserialized, connection_keep_alive);
    }

    #[test]
    fn prefix_sequence() {
        let packet_type = Packet::Disconnect.id();
        let sequence = 99999;

        let mut buffer = vec![];
        write_sequence(&mut buffer, sequence).unwrap();

        let prefix = encode_prefix(packet_type, sequence);
        let (d_packet_type, sequence_len) = decode_prefix(prefix);
        assert_eq!(packet_type, d_packet_type);
        assert_eq!(buffer.len(), sequence_len);

        let d_sequence = read_sequence(&mut buffer.as_slice(), sequence_len).unwrap();

        assert_eq!(sequence, d_sequence);
    }

    #[test]
    fn encrypt_decrypt_disconnect_packet() {
        let mut buffer = [0u8; NETCODE_MAX_PACKET_BYTES];
        let key = b"an example very very secret key."; // 32-bytes
        let packet = Packet::Disconnect;
        let protocol_id = 12;
        let sequence = 1;
        let len = packet
            .encode(&mut buffer, protocol_id, Some((sequence, key)))
            .unwrap();
        let (d_sequence, d_packet) =
            Packet::decode(&mut buffer[..len], protocol_id, Some(key), None).unwrap();
        assert_eq!(sequence, d_sequence);
        assert_eq!(packet, d_packet);
    }

    #[test]
    fn encrypt_decrypt_denied_packet() {
        let mut buffer = [0u8; NETCODE_MAX_PACKET_BYTES];
        let key = b"an example very very secret key."; // 32-bytes
        let packet = Packet::ConnectionDenied;
        let protocol_id = 12;
        let sequence = 2;
        let len = packet
            .encode(&mut buffer, protocol_id, Some((sequence, key)))
            .unwrap();
        let (d_sequence, d_packet) =
            Packet::decode(&mut buffer[..len], protocol_id, Some(key), None).unwrap();
        assert_eq!(sequence, d_sequence);
        assert_eq!(packet, d_packet);
    }

    #[test]
    fn encrypt_decrypt_payload_packet() {
        let mut buffer = [0u8; NETCODE_MAX_PACKET_BYTES];
        let payload = vec![7u8; NETCODE_MAX_PAYLOAD_BYTES];
        let key = b"an example very very secret key."; // 32-bytes
        let packet = Packet::Payload(&payload);
        let protocol_id = 12;
        let sequence = 2;
        let len = packet
            .encode(&mut buffer, protocol_id, Some((sequence, key)))
            .unwrap();
        let (d_sequence, d_packet) =
            Packet::decode(&mut buffer[..len], protocol_id, Some(key), None).unwrap();
        assert_eq!(sequence, d_sequence);
        match d_packet {
            Packet::Payload(ref p) => assert_eq!(&payload, p),
            _ => unreachable!(),
        }
        assert_eq!(packet, d_packet);
    }

    #[test]
    fn encrypt_decrypt_challenge_token() {
        let client_id = 0;
        let user_data = generate_random_bytes();
        let challenge_key = generate_random_bytes();
        let challenge_sequence = 1;
        let token = ChallengeToken::new(client_id, &user_data);
        let packet =
            Packet::generate_challenge(client_id, &user_data, challenge_sequence, &challenge_key)
                .unwrap();

        match packet {
            Packet::Challenge {
                token_data,
                token_sequence,
            } => {
                let decoded =
                    ChallengeToken::decode(token_data, token_sequence, &challenge_key).unwrap();
                assert_eq!(decoded, token);
            }
            _ => unreachable!(),
        }
    }
}
