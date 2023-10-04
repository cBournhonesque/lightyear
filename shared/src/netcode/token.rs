use std::time::Instant;
use std::{
    error::Error,
    fmt,
    io::{self, Cursor},
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    time::Duration,
};

use super::{
    constants::*,
    crypto::{dencrypted_in_place_xnonce, encrypt_in_place_xnonce, generate_random_bytes},
    serialize::*,
};
use crate::netcode::constants::{
    NETCODE_CONNECT_TOKEN_PRIVATE_BYTES, NETCODE_KEY_BYTES, NETCODE_USER_DATA_BYTES,
};
use crate::netcode::error::NetcodeError;
use chacha20poly1305::aead::Error as CryptoError;

/// A public connect token that the client receives to start connecting to the server.
/// How the client receives ConnectToken is up to you, could be from a matchmaking
/// system or from a call to a REST API as an example.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectToken {
    // NOTE: On the netcode.io standard the client id is not available in the public part of the
    // ConnectToken. But having it accessible here makes it easier to consume the token, and the
    // server still uses the client_id from the private part.
    pub(crate) client_id: u64,
    pub(crate) version_info: [u8; 13],
    pub(crate) protocol_id: u64,
    pub(crate) create_timestamp: u64,
    pub(crate) expire_timestamp: u64,
    pub(crate) xnonce: [u8; NETCODE_CONNECT_TOKEN_XNONCE_BYTES],
    pub(crate) server_addresses: [Option<SocketAddr>; 32],
    pub(crate) client_to_server_key: [u8; NETCODE_KEY_BYTES],
    pub(crate) server_to_client_key: [u8; NETCODE_KEY_BYTES],
    pub(crate) private_data: [u8; NETCODE_CONNECT_TOKEN_PRIVATE_BYTES],
    pub(crate) timeout_seconds: i32,
}

/// Connect token prior to encryption
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct PrivateConnectToken {
    pub client_id: u64,       // globally unique identifier for an authenticated client
    pub timeout_seconds: i32, // timeout in seconds. negative values disable timeout (dev only)
    pub server_addresses: [Option<SocketAddr>; 32],
    pub client_to_server_key: [u8; NETCODE_KEY_BYTES],
    pub server_to_client_key: [u8; NETCODE_KEY_BYTES],
    pub user_data: [u8; NETCODE_USER_DATA_BYTES], // user defined data specific to this protocol id
}

#[derive(Debug)]
pub enum TokenGenerationError {
    /// The maximum number of address in the token is 32
    MaxHostCount,
    CryptoError,
    IoError(io::Error),
    NoServerAddressAvailable,
}

impl From<io::Error> for TokenGenerationError {
    fn from(inner: io::Error) -> Self {
        TokenGenerationError::IoError(inner)
    }
}

impl From<CryptoError> for TokenGenerationError {
    fn from(_: CryptoError) -> Self {
        TokenGenerationError::CryptoError
    }
}

impl Error for TokenGenerationError {}

impl fmt::Display for TokenGenerationError {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        use TokenGenerationError::*;

        match *self {
            MaxHostCount => write!(fmt, "connect token can only have 32 server adresses"),
            CryptoError => write!(fmt, "error while encoding or decoding the connect token"),
            IoError(ref io_err) => write!(fmt, "{}", io_err),
            NoServerAddressAvailable => {
                write!(fmt, "connect token must have at least one server address")
            }
        }
    }
}

impl ConnectToken {
    /// Generate a token to be sent to an client. The user data is available to the server after an
    /// successful connection. The private key and the protocol id must be the same used in server.
    #[allow(clippy::too_many_arguments)]
    pub fn generate(
        current_time: Duration,
        protocol_id: u64,
        expire_seconds: u64,
        client_id: u64,
        timeout_seconds: i32,
        server_addresses: Vec<SocketAddr>,
        user_data: Option<&[u8; NETCODE_USER_DATA_BYTES]>,
        private_key: &[u8; NETCODE_KEY_BYTES],
    ) -> Result<Self, TokenGenerationError> {
        let expire_timestamp = current_time.as_secs() + expire_seconds;

        let private_connect_token =
            PrivateConnectToken::generate(client_id, timeout_seconds, server_addresses, user_data)?;
        let mut private_data = [0u8; NETCODE_CONNECT_TOKEN_PRIVATE_BYTES];
        let xnonce = generate_random_bytes();
        private_connect_token.encode(
            &mut private_data,
            protocol_id,
            expire_timestamp,
            &xnonce,
            private_key,
        )?;

        Ok(Self {
            client_id,
            version_info: *NETCODE_VERSION_INFO,
            protocol_id,
            private_data,
            create_timestamp: current_time.as_secs(),
            expire_timestamp,
            xnonce,
            server_addresses: private_connect_token.server_addresses,
            client_to_server_key: private_connect_token.client_to_server_key,
            server_to_client_key: private_connect_token.server_to_client_key,
            timeout_seconds: NETCODE_TIMEOUT_SECONDS,
        })
    }

    pub fn write(&self, writer: &mut impl io::Write) -> Result<(), io::Error> {
        writer.write_all(&self.client_id.to_le_bytes())?;
        writer.write_all(&self.version_info)?;
        writer.write_all(&self.protocol_id.to_le_bytes())?;
        writer.write_all(&self.create_timestamp.to_le_bytes())?;
        writer.write_all(&self.expire_timestamp.to_le_bytes())?;
        writer.write_all(&self.xnonce)?;
        writer.write_all(&self.private_data)?;
        writer.write_all(&self.timeout_seconds.to_le_bytes())?;
        write_server_adresses(writer, &self.server_addresses)?;
        writer.write_all(&self.client_to_server_key)?;
        writer.write_all(&self.server_to_client_key)?;

        Ok(())
    }

    pub fn read(src: &mut impl io::Read) -> Result<Self, NetcodeError> {
        let client_id = read_u64(src)?;
        let version_info: [u8; 13] = read_bytes(src)?;
        if &version_info != NETCODE_VERSION_INFO {
            return Err(NetcodeError::InvalidVersion);
        }

        let protocol_id = read_u64(src)?;
        let create_timestamp = read_u64(src)?;
        let expire_timestamp = read_u64(src)?;
        let xnonce = read_bytes(src)?;

        let private_data: [u8; NETCODE_CONNECT_TOKEN_PRIVATE_BYTES] = read_bytes(src)?;
        let timeout_seconds = read_i32(src)?;
        let server_addresses = read_server_addresses(src)?;
        let client_to_server_key: [u8; NETCODE_KEY_BYTES] = read_bytes(src)?;
        let server_to_client_key: [u8; NETCODE_KEY_BYTES] = read_bytes(src)?;

        Ok(Self {
            client_id,
            version_info,
            protocol_id,
            create_timestamp,
            expire_timestamp,
            xnonce,
            private_data,
            server_addresses,
            client_to_server_key,
            server_to_client_key,
            timeout_seconds,
        })
    }
}

impl PrivateConnectToken {
    fn generate(
        client_id: u64,
        timeout_seconds: i32,
        server_addresses: Vec<SocketAddr>,
        user_data: Option<&[u8; NETCODE_USER_DATA_BYTES]>,
    ) -> Result<Self, TokenGenerationError> {
        if server_addresses.len() > 32 {
            return Err(TokenGenerationError::MaxHostCount);
        }
        if server_addresses.is_empty() {
            return Err(TokenGenerationError::NoServerAddressAvailable);
        }

        let mut server_addresses_arr = [None; 32];
        for (i, addr) in server_addresses.into_iter().enumerate() {
            server_addresses_arr[i] = Some(addr);
        }

        let client_to_server_key = generate_random_bytes();
        let server_to_client_key = generate_random_bytes();

        let user_data = match user_data {
            Some(data) => *data,
            None => generate_random_bytes(),
        };

        Ok(Self {
            client_id,
            timeout_seconds,
            server_addresses: server_addresses_arr,
            client_to_server_key,
            server_to_client_key,
            user_data,
        })
    }

    fn write(&self, writer: &mut impl io::Write) -> Result<(), io::Error> {
        writer.write_all(&self.client_id.to_le_bytes())?;
        writer.write_all(&self.timeout_seconds.to_le_bytes())?;
        write_server_adresses(writer, &self.server_addresses)?;
        writer.write_all(&self.client_to_server_key)?;
        writer.write_all(&self.server_to_client_key)?;
        writer.write_all(&self.user_data)?;

        Ok(())
    }

    fn read(src: &mut impl io::Read) -> Result<Self, io::Error> {
        let client_id = read_u64(src)?;
        let timeout_seconds = read_i32(src)?;
        let server_addresses = read_server_addresses(src)?;
        let mut client_to_server_key = [0u8; 32];
        src.read_exact(&mut client_to_server_key)?;

        let mut server_to_client_key = [0u8; 32];
        src.read_exact(&mut server_to_client_key)?;

        let mut user_data = [0u8; 256];
        src.read_exact(&mut user_data)?;

        Ok(Self {
            client_id,
            timeout_seconds,
            server_addresses,
            client_to_server_key,
            server_to_client_key,
            user_data,
        })
    }

    pub(crate) fn encode(
        &self,
        buffer: &mut [u8; NETCODE_CONNECT_TOKEN_PRIVATE_BYTES],
        protocol_id: u64,
        expire_timestamp: u64,
        xnonce: &[u8; NETCODE_CONNECT_TOKEN_XNONCE_BYTES],
        private_key: &[u8; NETCODE_KEY_BYTES],
    ) -> Result<(), TokenGenerationError> {
        let aad = get_associated_data_for_encryption(protocol_id, expire_timestamp);
        self.write(&mut Cursor::new(&mut buffer[..]))?;

        encrypt_in_place_xnonce(buffer, xnonce, private_key, &aad)?;

        Ok(())
    }

    pub(crate) fn decode(
        buffer: &[u8; NETCODE_CONNECT_TOKEN_PRIVATE_BYTES],
        protocol_id: u64,
        expire_timestamp: u64,
        xnonce: &[u8; NETCODE_CONNECT_TOKEN_XNONCE_BYTES],
        private_key: &[u8; NETCODE_KEY_BYTES],
    ) -> Result<Self, TokenGenerationError> {
        let aad = get_associated_data_for_encryption(protocol_id, expire_timestamp);

        let mut temp_buffer = [0u8; NETCODE_CONNECT_TOKEN_PRIVATE_BYTES];
        temp_buffer.copy_from_slice(buffer);

        dencrypted_in_place_xnonce(&mut temp_buffer, xnonce, private_key, &aad)?;

        let src = &mut io::Cursor::new(&temp_buffer[..]);
        Ok(Self::read(src)?)
    }
}

fn write_server_adresses(
    writer: &mut impl io::Write,
    server_addresses: &[Option<SocketAddr>; 32],
) -> Result<(), io::Error> {
    let num_server_addresses: u32 = server_addresses.iter().filter(|a| a.is_some()).count() as u32;
    writer.write_all(&num_server_addresses.to_le_bytes())?;

    for host in server_addresses.iter().flatten() {
        match host {
            SocketAddr::V4(addr) => {
                writer.write_all(&NETCODE_ADDRESS_IPV4.to_le_bytes())?;
                for i in addr.ip().octets() {
                    writer.write_all(&i.to_le_bytes())?;
                }
            }
            SocketAddr::V6(addr) => {
                writer.write_all(&NETCODE_ADDRESS_IPV6.to_le_bytes())?;
                for i in addr.ip().octets() {
                    writer.write_all(&i.to_le_bytes())?;
                }
            }
        }
        writer.write_all(&host.port().to_le_bytes())?;
    }

    Ok(())
}

fn read_server_addresses(src: &mut impl io::Read) -> Result<[Option<SocketAddr>; 32], io::Error> {
    let mut server_addresses = [None; 32];
    let num_server_addresses = read_u32(src)? as usize;
    for server_address in server_addresses.iter_mut().take(num_server_addresses) {
        let host_type = read_u8(src)?;
        match host_type {
            NETCODE_ADDRESS_IPV4 => {
                let mut ip = [0u8; 4];
                src.read_exact(&mut ip)?;
                let port = read_u16(src)?;
                let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::from(ip)), port);
                *server_address = Some(addr);
            }
            NETCODE_ADDRESS_IPV6 => {
                let mut ip = [0u8; 16];
                src.read_exact(&mut ip)?;
                let port = read_u16(src)?;
                let addr = SocketAddr::new(IpAddr::V6(Ipv6Addr::from(ip)), port);
                *server_address = Some(addr);
            }
            NETCODE_ADDRESS_NONE => {} // skip
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Unknown ip address type",
                ))
            }
        }
    }

    if server_addresses.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "ConnectToken does not have a server address",
        ));
    }

    Ok(server_addresses)
}

// Get the as
fn get_associated_data_for_encryption(
    protocol_id: u64,
    expire_timestamp: u64,
) -> [u8; NETCODE_ADDITIONAL_DATA_SIZE] {
    let mut buffer = [0; NETCODE_ADDITIONAL_DATA_SIZE];
    buffer[..13].copy_from_slice(NETCODE_VERSION_INFO);
    buffer[13..21].copy_from_slice(&protocol_id.to_le_bytes());
    buffer[21..29].copy_from_slice(&expire_timestamp.to_le_bytes());

    buffer
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_connect_token_serialization() {
        let hosts: Vec<SocketAddr> = vec![
            "127.0.0.1:8080".parse().unwrap(),
            "127.0.0.2:3000".parse().unwrap(),
        ];
        let token =
            PrivateConnectToken::generate(1, 5, hosts, Some(&generate_random_bytes())).unwrap();
        let mut buffer: Vec<u8> = vec![];

        token.write(&mut buffer).unwrap();
        let result = PrivateConnectToken::read(&mut buffer.as_slice()).unwrap();

        assert_eq!(token, result);
    }

    #[test]
    fn private_connect_token_encode_decode() {
        let hosts: Vec<SocketAddr> = vec![
            "127.0.0.1:8080".parse().unwrap(),
            "127.0.0.2:3000".parse().unwrap(),
        ];
        let token =
            PrivateConnectToken::generate(1, 5, hosts, Some(&generate_random_bytes())).unwrap();
        let key = b"an example very very secret key."; // 32-bytes
        let protocol_id = 12;
        let expire_timestamp = 0;
        let mut buffer = [0u8; NETCODE_CONNECT_TOKEN_PRIVATE_BYTES];
        let xnonce = generate_random_bytes();
        token
            .encode(&mut buffer, protocol_id, expire_timestamp, &xnonce, key)
            .unwrap();

        let result =
            PrivateConnectToken::decode(&buffer, protocol_id, expire_timestamp, &xnonce, key)
                .unwrap();
        assert_eq!(token, result);
    }

    #[test]
    fn connect_token_serialization() {
        let server_addresses: Vec<SocketAddr> = vec![
            "127.0.0.1:8080".parse().unwrap(),
            "127.0.0.2:3000".parse().unwrap(),
        ];
        let user_data = generate_random_bytes();
        let private_key = b"an example very very secret key."; // 32-bytes
        let protocol_id = 2;
        let expire_seconds = 3;
        let client_id = 4;
        let timeout_seconds = 5;
        let token = ConnectToken::generate(
            Duration::ZERO,
            protocol_id,
            expire_seconds,
            client_id,
            timeout_seconds,
            server_addresses,
            Some(&user_data),
            private_key,
        )
        .unwrap();

        let mut buffer: Vec<u8> = vec![];
        token.write(&mut buffer).unwrap();

        let result = ConnectToken::read(&mut buffer.as_slice()).unwrap();
        assert_eq!(token, result);

        let private = PrivateConnectToken::decode(
            &result.private_data,
            protocol_id,
            result.expire_timestamp,
            &result.xnonce,
            private_key,
        )
        .unwrap();
        assert_eq!(timeout_seconds, private.timeout_seconds);
        assert_eq!(client_id, private.client_id);
        assert_eq!(user_data, private.user_data);
        assert_eq!(token.server_addresses, private.server_addresses);
        assert_eq!(token.client_to_server_key, private.client_to_server_key);
        assert_eq!(token.server_to_client_key, private.server_to_client_key);
    }
}
