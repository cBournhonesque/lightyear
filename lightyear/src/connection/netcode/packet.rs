use std::{
    io::{self, Read, Write},
    mem::size_of,
};

use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use chacha20poly1305::XNonce;
use tracing::debug;

use crate::connection::netcode::ClientId;

use super::{
    bytes::Bytes,
    crypto::{self, Key},
    error::Error as NetcodeError,
    replay::ReplayProtection,
    token::{ChallengeToken, ConnectTokenPrivate},
    MAC_BYTES, MAX_PKT_BUF_SIZE, NETCODE_VERSION,
};

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("packet type {0} is invalid")]
    InvalidType(u8),
    #[error("sequence bytes {0} are out of range [1, 8]")]
    InvalidSequenceBytes(u8),
    #[error("packet length is less than 1")]
    TooSmall,
    #[error("packet length is greater than 1200")]
    TooLarge,
    #[error("bad packet length, expected {expected} but got {actual}")]
    LengthMismatch { expected: usize, actual: usize },
    #[error("bad version info")]
    BadVersion,
    #[error("wrong protocol id, expected {expected} but got {actual}")]
    BadProtocolId { expected: u64, actual: u64 },
    #[error("connect token expired")]
    TokenExpired,
    #[error("sequence {0} already received")]
    AlreadyReceived(u64),
    #[error("invalid packet payload")]
    InvalidPayload,
}

trait WriteSequence {
    fn write_sequence(&mut self, sequence: u64) -> Result<(), io::Error>;
}

trait ReadSequence {
    fn read_sequence(&mut self, sequence_len: usize) -> Result<u64, io::Error>;
}

impl<W> WriteSequence for W
where
    W: Write,
{
    fn write_sequence(&mut self, sequence: u64) -> Result<(), io::Error> {
        let sequence_len = sequence_len(sequence);
        for shift in 0..sequence_len {
            self.write_u8(((sequence >> (shift * 8) as u64) & 0xFF) as u8)?;
        }
        Ok(())
    }
}

impl<R> ReadSequence for R
where
    R: Read,
{
    fn read_sequence(&mut self, sequence_len: usize) -> Result<u64, io::Error> {
        let mut sequence = [0; 8];
        self.read_exact(&mut sequence[..sequence_len])?;
        Ok(u64::from_le_bytes(sequence))
    }
}

pub struct RequestPacket {
    pub version_info: [u8; NETCODE_VERSION.len()],
    pub protocol_id: u64,
    pub expire_timestamp: u64,
    pub token_nonce: XNonce,
    pub token_data: Box<[u8; ConnectTokenPrivate::SIZE]>,
}

impl RequestPacket {
    pub fn create(
        protocol_id: u64,
        expire_timestamp: u64,
        token_nonce: XNonce,
        token_data: [u8; ConnectTokenPrivate::SIZE],
    ) -> Packet<'static> {
        Packet::Request(RequestPacket {
            version_info: *NETCODE_VERSION,
            protocol_id,
            expire_timestamp,
            token_nonce,
            token_data: Box::new(token_data),
        })
    }
    pub fn validate(&self, protocol_id: u64, current_timestamp: u64) -> Result<(), Error> {
        if &self.version_info != NETCODE_VERSION {
            return Err(Error::BadVersion);
        }
        if self.protocol_id != protocol_id {
            return Err(Error::BadProtocolId {
                expected: protocol_id,
                actual: self.protocol_id,
            });
        }
        if self.expire_timestamp <= current_timestamp {
            return Err(Error::TokenExpired);
        }
        Ok(())
    }

    pub fn decrypt_token_data(&mut self, private_key: Key) -> Result<(), NetcodeError> {
        let decrypted = ConnectTokenPrivate::decrypt(
            &mut self.token_data[..],
            self.protocol_id,
            self.expire_timestamp,
            self.token_nonce,
            &private_key,
        )?;
        let mut token_data = std::io::Cursor::new(&mut self.token_data[..]);
        decrypted.write_to(&mut token_data)?;
        Ok(())
    }
}

impl Bytes for RequestPacket {
    type Error = io::Error;
    fn write_to(&self, writer: &mut impl WriteBytesExt) -> Result<(), Self::Error> {
        writer.write_all(&self.version_info)?;
        writer.write_u64::<LittleEndian>(self.protocol_id)?;
        writer.write_u64::<LittleEndian>(self.expire_timestamp)?;
        writer.write_all(&self.token_nonce)?;
        writer.write_all(&self.token_data[..])?;
        Ok(())
    }

    fn read_from(reader: &mut impl byteorder::ReadBytesExt) -> Result<Self, io::Error> {
        let mut version_info = [0; NETCODE_VERSION.len()];
        reader.read_exact(&mut version_info)?;
        let protocol_id = reader.read_u64::<LittleEndian>()?;
        let expire_timestamp = reader.read_u64::<LittleEndian>()?;
        let mut nonce = [0; size_of::<XNonce>()];
        reader.read_exact(&mut nonce)?;
        let token_nonce = XNonce::from_slice(&nonce).to_owned();
        let mut token_data = [0; ConnectTokenPrivate::SIZE];
        reader.read_exact(&mut token_data)?;
        Ok(Self {
            version_info,
            protocol_id,
            expire_timestamp,
            token_nonce,
            token_data: Box::new(token_data),
        })
    }
}

pub struct DeniedPacket {}

impl DeniedPacket {
    pub fn create() -> Packet<'static> {
        Packet::Denied(DeniedPacket {})
    }
}

impl Bytes for DeniedPacket {
    type Error = io::Error;
    fn write_to(&self, _writer: &mut impl WriteBytesExt) -> Result<(), Self::Error> {
        Ok(())
    }

    fn read_from(_reader: &mut impl byteorder::ReadBytesExt) -> Result<Self, io::Error> {
        Ok(Self {})
    }
}

pub struct ChallengePacket {
    pub sequence: u64,
    pub token: [u8; ChallengeToken::SIZE],
}

impl ChallengePacket {
    pub fn create(sequence: u64, token_bytes: [u8; ChallengeToken::SIZE]) -> Packet<'static> {
        Packet::Challenge(ChallengePacket {
            sequence,
            token: token_bytes,
        })
    }
}

impl Bytes for ChallengePacket {
    type Error = io::Error;
    fn write_to(&self, writer: &mut impl WriteBytesExt) -> Result<(), Self::Error> {
        writer.write_u64::<LittleEndian>(self.sequence)?;
        writer.write_all(&self.token)?;
        Ok(())
    }

    fn read_from(reader: &mut impl byteorder::ReadBytesExt) -> Result<Self, io::Error> {
        let sequence = reader.read_u64::<LittleEndian>()?;
        let mut token = [0; ChallengeToken::SIZE];
        reader.read_exact(&mut token)?;
        Ok(Self { sequence, token })
    }
}

pub struct ResponsePacket {
    pub sequence: u64,
    pub token: [u8; ChallengeToken::SIZE],
}

impl ResponsePacket {
    pub fn create(sequence: u64, token_bytes: [u8; ChallengeToken::SIZE]) -> Packet<'static> {
        Packet::Response(ResponsePacket {
            sequence,
            token: token_bytes,
        })
    }
}

impl Bytes for ResponsePacket {
    type Error = io::Error;
    fn write_to(&self, writer: &mut impl WriteBytesExt) -> Result<(), Self::Error> {
        writer.write_u64::<LittleEndian>(self.sequence)?;
        writer.write_all(&self.token)?;
        Ok(())
    }

    fn read_from(reader: &mut impl byteorder::ReadBytesExt) -> Result<Self, io::Error> {
        let sequence = reader.read_u64::<LittleEndian>()?;
        let mut token = [0; ChallengeToken::SIZE];
        reader.read_exact(&mut token)?;
        Ok(Self { sequence, token })
    }
}

pub struct KeepAlivePacket {
    pub client_id: ClientId,
}

impl KeepAlivePacket {
    pub fn create(client_id: ClientId) -> Packet<'static> {
        Packet::KeepAlive(KeepAlivePacket { client_id })
    }
}

impl Bytes for KeepAlivePacket {
    type Error = io::Error;
    fn write_to(&self, writer: &mut impl WriteBytesExt) -> Result<(), Self::Error> {
        writer.write_u64::<LittleEndian>(self.client_id)?;
        Ok(())
    }

    fn read_from(reader: &mut impl byteorder::ReadBytesExt) -> Result<Self, io::Error> {
        let client_id = reader.read_u64::<LittleEndian>()?;
        Ok(Self { client_id })
    }
}

pub struct PayloadPacket<'p> {
    pub buf: &'p [u8],
}

impl PayloadPacket<'_> {
    pub fn create(buf: &[u8]) -> Packet {
        Packet::Payload(PayloadPacket { buf })
    }
}

pub struct DisconnectPacket {}

impl DisconnectPacket {
    pub fn create() -> Packet<'static> {
        Packet::Disconnect(Self {})
    }
}

impl Bytes for DisconnectPacket {
    type Error = io::Error;
    fn write_to(&self, _writer: &mut impl WriteBytesExt) -> Result<(), Self::Error> {
        Ok(())
    }

    fn read_from(_reader: &mut impl byteorder::ReadBytesExt) -> Result<Self, io::Error> {
        Ok(Self {})
    }
}

pub enum Packet<'p> {
    Request(RequestPacket),
    Denied(DeniedPacket),
    Challenge(ChallengePacket),
    Response(ResponsePacket),
    KeepAlive(KeepAlivePacket),
    Payload(PayloadPacket<'p>),
    Disconnect(DisconnectPacket),
}

impl std::fmt::Display for Packet<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Packet::Request(_) => write!(f, "connection request"),
            Packet::Response(_) => write!(f, "connection response"),
            Packet::KeepAlive(_) => write!(f, "keep-alive packet"),
            Packet::Payload(_) => write!(f, "payload packet"),
            Packet::Disconnect(_) => write!(f, "disconnect packet"),
            Packet::Denied(_) => write!(f, "denied packet"),
            Packet::Challenge(_) => write!(f, "challenge packet"),
        }
    }
}

pub type PacketKind = u8;

impl<'p> Packet<'p> {
    pub const REQUEST: PacketKind = 0;
    pub const DENIED: PacketKind = 1;
    pub const CHALLENGE: PacketKind = 2;
    pub const RESPONSE: PacketKind = 3;
    pub const KEEP_ALIVE: PacketKind = 4;
    pub const PAYLOAD: PacketKind = 5;
    pub const DISCONNECT: PacketKind = 6;
    fn kind(&self) -> PacketKind {
        match self {
            Packet::Request(_) => Packet::REQUEST,
            Packet::Denied(_) => Packet::DENIED,
            Packet::Challenge(_) => Packet::CHALLENGE,
            Packet::Response(_) => Packet::RESPONSE,
            Packet::KeepAlive(_) => Packet::KEEP_ALIVE,
            Packet::Payload(_) => Packet::PAYLOAD,
            Packet::Disconnect(_) => Packet::DISCONNECT,
        }
    }
    fn set_prefix(&self, sequence: u64) -> u8 {
        sequence_len(sequence) << 4 | self.kind()
    }
    fn aead(
        protocol_id: u64,
        prefix: u8,
    ) -> Result<[u8; NETCODE_VERSION.len() + size_of::<u64>() + size_of::<u8>()], NetcodeError>
    {
        // Encrypt the per-packet packet written with the prefix byte, protocol id and version as the associated data.
        // This must match to decrypt.
        let mut aead = [0u8; NETCODE_VERSION.len() + size_of::<u64>() + size_of::<u8>()];
        let mut cursor = std::io::Cursor::new(&mut aead[..]);
        cursor.write_all(NETCODE_VERSION).unwrap();
        cursor.write_u64::<LittleEndian>(protocol_id).unwrap();
        cursor.write_u8(prefix).unwrap();
        Ok(aead)
    }
    pub fn get_prefix(prefix_byte: u8) -> (usize, PacketKind) {
        ((prefix_byte >> 4) as usize, prefix_byte & 0xF)
    }
    pub fn write(
        &self,
        out: &mut [u8],
        sequence: u64,
        packet_key: &Key,
        protocol_id: u64,
    ) -> Result<usize, NetcodeError> {
        let len = out.len();
        let mut cursor = std::io::Cursor::new(&mut out[..]);
        if let Packet::Request(pkt) = self {
            cursor.write_u8(Packet::REQUEST)?;
            pkt.write_to(&mut cursor)?;
            return Ok(cursor.position() as usize);
        }
        cursor.write_u8(self.set_prefix(sequence))?;
        cursor.write_sequence(sequence)?;
        let encryption_start = cursor.position() as usize;
        match self {
            Packet::Denied(pkt) => pkt.write_to(&mut cursor)?,
            Packet::Challenge(pkt) => pkt.write_to(&mut cursor)?,
            Packet::Response(pkt) => pkt.write_to(&mut cursor)?,
            Packet::KeepAlive(pkt) => pkt.write_to(&mut cursor)?,
            Packet::Disconnect(pkt) => pkt.write_to(&mut cursor)?,
            Packet::Payload(PayloadPacket { buf }) => cursor.write_all(buf)?,
            _ => unreachable!(), // Packet::Request variant is handled above
        }
        if cursor.position() as usize > len - MAC_BYTES {
            return Err(Error::TooLarge.into());
        }
        let encryption_end = cursor.position() as usize + MAC_BYTES;

        crypto::chacha_encrypt(
            &mut out[encryption_start..encryption_end],
            Some(&Packet::aead(protocol_id, self.set_prefix(sequence))?),
            sequence,
            packet_key,
        )?;

        Ok(encryption_end)
    }
    pub fn read(
        buf: &'p mut [u8], // buffer needs to be mutable to perform decryption in-place
        protocol_id: u64,
        timestamp: u64,
        key: Key,
        replay_protection: Option<&mut ReplayProtection>,
        allowed_packets: u8,
    ) -> Result<Packet<'p>, NetcodeError> {
        let buf_len = buf.len();
        if buf_len < 1 {
            return Err(Error::TooSmall.into());
        }
        if buf_len > MAX_PKT_BUF_SIZE {
            return Err(Error::TooLarge.into());
        }
        let mut cursor = std::io::Cursor::new(&mut buf[..]);
        let prefix_byte = cursor.read_u8()?;
        let (sequence_len, pkt_kind) = Packet::get_prefix(prefix_byte);
        if allowed_packets & (1 << pkt_kind) == 0 {
            debug!("ignoring packet of type {}, not allowed", pkt_kind);
        }
        if prefix_byte == Packet::REQUEST {
            // connection request packet: first byte should be 0x00
            let mut packet = RequestPacket::read_from(&mut cursor)?;
            packet.validate(protocol_id, timestamp)?;
            packet.decrypt_token_data(key)?;
            return Ok(Packet::Request(packet));
        }
        if buf_len < size_of::<u8>() + sequence_len + MAC_BYTES {
            // should at least have prefix byte, sequence and mac
            return Err(Error::TooSmall.into());
        }
        let sequence = cursor.read_sequence(sequence_len)?;

        // Replay protection
        if let Some(replay_protection) = replay_protection.as_ref() {
            if pkt_kind >= Packet::KEEP_ALIVE && replay_protection.is_already_received(sequence) {
                return Err(Error::AlreadyReceived(sequence).into());
            }
        }

        let decryption_start = cursor.position() as usize;
        let decryption_end = buf_len;
        crypto::chacha_decrypt(
            &mut cursor.get_mut()[decryption_start..decryption_end],
            Some(&Packet::aead(protocol_id, prefix_byte)?),
            sequence,
            &key,
        )?;
        // make sure cursor position is at the start of the decrypted data, so we can read it into a valid packet
        cursor.set_position(decryption_start as u64);

        if let Some(replay_protection) = replay_protection {
            if pkt_kind >= Packet::KEEP_ALIVE {
                replay_protection.advance_sequence(sequence);
            }
        }

        let packet = match pkt_kind {
            Packet::REQUEST => Packet::Request(RequestPacket::read_from(&mut cursor)?),
            Packet::DENIED => Packet::Denied(DeniedPacket::read_from(&mut cursor)?),
            Packet::CHALLENGE => Packet::Challenge(ChallengePacket::read_from(&mut cursor)?),
            Packet::RESPONSE => Packet::Response(ResponsePacket::read_from(&mut cursor)?),
            Packet::KEEP_ALIVE => Packet::KeepAlive(KeepAlivePacket::read_from(&mut cursor)?),
            Packet::DISCONNECT => Packet::Disconnect(DisconnectPacket::read_from(&mut cursor)?),
            Packet::PAYLOAD => {
                buf.copy_within(decryption_start..(decryption_end - MAC_BYTES), 0);
                Packet::Payload(PayloadPacket {
                    buf: &buf[..decryption_end - decryption_start - MAC_BYTES],
                })
            }
            t => return Err(Error::InvalidType(t).into()),
        };
        Ok(packet)
    }
}

pub fn sequence_len(sequence: u64) -> u8 {
    std::cmp::max(8 - sequence.leading_zeros() as u8 / 8, 1)
}

#[cfg(test)]
mod tests {
    use chacha20poly1305::{aead::OsRng, AeadCore, XChaCha20Poly1305};

    use crate::connection::netcode::{
        crypto::generate_key, token::AddressList, MAX_PACKET_SIZE, USER_DATA_BYTES,
    };

    use super::*;

    #[test]
    fn sequence_number_bytes_required() {
        assert_eq!(sequence_len(0), 1);
        assert_eq!(sequence_len(1), 1);
        assert_eq!(sequence_len(0x1_00), 2);
        assert_eq!(sequence_len(0x1_00_00), 3);
        assert_eq!(sequence_len(0x1_00_00_00), 4);
        assert_eq!(sequence_len(0x1_00_00_00_00), 5);
        assert_eq!(sequence_len(0x1_00_00_00_00_00), 6);
        assert_eq!(sequence_len(0x1_00_00_00_00_00_00), 7);
        assert_eq!(sequence_len(0x1_00_00_00_00_00_00_00), 8);
        assert_eq!(sequence_len(0x80_00_00_00_00_00_00_00), 8);

        let sequence = 1u64 << 63;
        let cursor = &mut std::io::Cursor::new(Vec::new());
        cursor.write_sequence(sequence).unwrap();
        assert_eq!(cursor.get_ref().len(), 8);
        cursor.set_position(0);
        assert_eq!(cursor.read_sequence(8).unwrap(), sequence);
    }

    #[test]
    fn request_packet() {
        let client_id = 0x1234;
        let timeout_seconds = -1;
        let server_addresses = AddressList::new("127.0.0.1:40002").unwrap();
        let user_data = [0u8; USER_DATA_BYTES];
        let private_key = generate_key();
        let packet_key = generate_key();
        let protocol_id = 0x1234_5678_9abc_def0;
        let expire_timestamp = u64::MAX;
        let nonce = XChaCha20Poly1305::generate_nonce(&mut OsRng);
        let sequence = 0u64;
        let mut replay_protection = ReplayProtection::new();
        let token_data = ConnectTokenPrivate {
            client_id,
            timeout_seconds,
            server_addresses,
            user_data,
            client_to_server_key: generate_key(),
            server_to_client_key: generate_key(),
        };

        let token_data = token_data
            .encrypt(protocol_id, expire_timestamp, nonce, &private_key)
            .unwrap();

        let packet = Packet::Request(RequestPacket {
            version_info: *NETCODE_VERSION,
            protocol_id,
            expire_timestamp,
            token_nonce: nonce,
            token_data: Box::new(token_data),
        });

        let mut buf = [0u8; MAX_PACKET_SIZE];
        let size = packet
            .write(&mut buf, sequence, &packet_key, protocol_id)
            .unwrap();

        let packet = Packet::read(
            &mut buf[..size],
            protocol_id,
            0,
            private_key,
            Some(&mut replay_protection),
            0xff,
        )
        .unwrap();

        let Packet::Request(req_pkt) = packet else {
            panic!("wrong packet type");
        };

        assert_eq!(req_pkt.version_info, *NETCODE_VERSION);
        assert_eq!(req_pkt.protocol_id, protocol_id);
        assert_eq!(req_pkt.expire_timestamp, expire_timestamp);
        assert_eq!(req_pkt.token_nonce, nonce);

        let mut reader = std::io::Cursor::new(&req_pkt.token_data[..]);
        let connect_token_private = ConnectTokenPrivate::read_from(&mut reader).unwrap();
        assert_eq!(connect_token_private.client_id, client_id);
        assert_eq!(connect_token_private.timeout_seconds, timeout_seconds);
        connect_token_private
            .server_addresses
            .iter()
            .zip(server_addresses.iter())
            .for_each(|(have, expected)| {
                assert_eq!(have, expected);
            });
        assert_eq!(connect_token_private.user_data, user_data);
    }

    #[test]
    fn denied_packet() {
        let packet_key = generate_key();
        let protocol_id = 0x1234_5678_9abc_def0;
        let sequence = 0u64;
        let mut replay_protection = ReplayProtection::new();

        let packet = Packet::Denied(DeniedPacket {});

        let mut buf = [0u8; MAX_PKT_BUF_SIZE];
        let size = packet
            .write(&mut buf, sequence, &packet_key, protocol_id)
            .unwrap();

        let packet = Packet::read(
            &mut buf[..size],
            protocol_id,
            0,
            packet_key,
            Some(&mut replay_protection),
            0xff,
        )
        .unwrap();

        let Packet::Denied(_denied_pkt) = packet else {
            panic!("wrong packet type");
        };
    }

    #[test]
    pub fn challenge_packet() {
        let token = [0u8; ChallengeToken::SIZE];
        let packet_key = generate_key();
        let protocol_id = 0x1234_5678_9abc_def0;
        let sequence = 0u64;
        let mut replay_protection = ReplayProtection::new();

        let packet = Packet::Challenge(ChallengePacket { sequence, token });

        let mut buf = [0u8; MAX_PKT_BUF_SIZE];
        let size = packet
            .write(&mut buf, sequence, &packet_key, protocol_id)
            .unwrap();

        let packet = Packet::read(
            &mut buf[..size],
            protocol_id,
            0,
            packet_key,
            Some(&mut replay_protection),
            0xff,
        )
        .unwrap();

        let Packet::Challenge(challenge_pkt) = packet else {
            panic!("wrong packet type");
        };

        assert_eq!(challenge_pkt.token, token);
        assert_eq!(challenge_pkt.sequence, sequence);
    }

    #[test]
    pub fn keep_alive_packet() {
        let packet_key = generate_key();
        let protocol_id = 0x1234_5678_9abc_def0;
        let sequence = 0u64;
        let client_id = 0x1234;
        let mut replay_protection = ReplayProtection::new();

        let packet = Packet::KeepAlive(KeepAlivePacket { client_id });

        let mut buf = [0u8; MAX_PKT_BUF_SIZE];
        let size = packet
            .write(&mut buf, sequence, &packet_key, protocol_id)
            .unwrap();

        let packet = Packet::read(
            &mut buf[..size],
            protocol_id,
            0,
            packet_key,
            Some(&mut replay_protection),
            0xff,
        )
        .unwrap();

        let Packet::KeepAlive(keep_alive_pkt) = packet else {
            panic!("wrong packet type");
        };

        assert_eq!(keep_alive_pkt.client_id, client_id);
    }

    #[test]
    pub fn disconnect_packet() {
        let packet_key = generate_key();
        let protocol_id = 0x1234_5678_9abc_def0;
        let sequence = 0u64;
        let mut replay_protection = ReplayProtection::new();

        let packet = Packet::Disconnect(DisconnectPacket {});

        let mut buf = [0u8; MAX_PKT_BUF_SIZE];
        let size = packet
            .write(&mut buf, sequence, &packet_key, protocol_id)
            .unwrap();

        let packet = Packet::read(
            &mut buf[..size],
            protocol_id,
            0,
            packet_key,
            Some(&mut replay_protection),
            0xff,
        )
        .unwrap();

        let Packet::Disconnect(_disconnect_pkt) = packet else {
            panic!("wrong packet type");
        };
    }

    #[test]
    pub fn payload_packet() {
        let packet_key = generate_key();
        let protocol_id = 0x1234_5678_9abc_def0;
        let sequence = 0u64;
        let mut replay_protection = ReplayProtection::new();

        let payload = vec![0u8; 100];
        let packet = Packet::Payload(PayloadPacket { buf: &payload });

        let mut buf = [0u8; MAX_PACKET_SIZE];
        let size = packet
            .write(&mut buf, sequence, &packet_key, protocol_id)
            .unwrap();

        let packet = Packet::read(
            &mut buf[..size],
            protocol_id,
            0,
            packet_key,
            Some(&mut replay_protection),
            0xff,
        )
        .unwrap();

        let Packet::Payload(data_pkt) = packet else {
            panic!("wrong packet type");
        };

        assert_eq!(data_pkt.buf.len(), 100);
    }
}
