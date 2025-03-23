use alloc::borrow::ToOwned;
use core::mem::size_of;
#[cfg(not(feature = "std"))]
use alloc::format;
use core::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use no_std_io2::io::{self, Write};
use chacha20poly1305::{aead::OsRng, AeadCore, XChaCha20Poly1305, XNonce};
use thiserror::Error;
use crate::serialize::reader::ReadInteger;
use crate::serialize::writer::WriteInteger;
use super::{
    bytes::Bytes,
    crypto::{self, Key},
    error::Error,
    utils, CONNECTION_TIMEOUT_SEC, CONNECT_TOKEN_BYTES, NETCODE_VERSION, PRIVATE_KEY_BYTES,
    USER_DATA_BYTES,
};
use crate::utils::free_list::{FreeList, FreeListIter};


const MAX_SERVERS_PER_CONNECT: usize = 32;
pub(crate) const TOKEN_EXPIRE_SEC: i32 = 30;

/// An error that can occur when de-serializing a connect token from bytes.
#[derive(Error, Debug)]
pub enum InvalidTokenError {
    #[error("address list length is out of range 1-32: {0}")]
    AddressListLength(u32),
    #[error("invalid ip address type (must be 1 for ipv4 or 2 for ipv6): {0}")]
    InvalidIpAddressType(u8),
    #[error("create timestamp is greater than expire timestamp")]
    InvalidTimestamp,
    #[error("invalid version")]
    InvalidVersion,
    #[error("io error: {0}")]
    Io(#[from] io::Error),
}

#[derive(Debug, Clone, Copy)]
pub struct AddressList {
    addrs: FreeList<SocketAddr, MAX_SERVERS_PER_CONNECT>,
}

impl AddressList {
    const IPV4: u8 = 1;
    const IPV6: u8 = 2;
    pub fn new(addrs: impl utils::ToSocketAddrs) -> Result<Self, Error> {
        let mut server_addresses = FreeList::new();

        for (i, addr) in addrs.to_socket_addrs()?.enumerate() {
            if i >= MAX_SERVERS_PER_CONNECT {
                break;
            }

            server_addresses.insert(addr);
        }

        Ok(AddressList {
            addrs: server_addresses,
        })
    }
    pub fn len(&self) -> usize {
        self.addrs.len()
    }
    pub fn iter(&self) -> FreeListIter<SocketAddr, MAX_SERVERS_PER_CONNECT> {
        FreeListIter {
            free_list: &self.addrs,
            index: 0,
        }
    }
}

impl core::ops::Index<usize> for AddressList {
    type Output = SocketAddr;

    fn index(&self, index: usize) -> &Self::Output {
        self.addrs.get(index).expect("index out of bounds")
    }
}

impl Bytes for AddressList {
    const SIZE: usize = size_of::<u32>() + MAX_SERVERS_PER_CONNECT * (1 + size_of::<u16>() + 16);
    type Error = InvalidTokenError;
    fn write_to(&self, buf: &mut impl WriteInteger) -> Result<(), InvalidTokenError> {
        buf.write_u32(self.len() as u32)?;
        for (_, addr) in self.iter() {
            match addr {
               SocketAddr::V4(addr_v4) => {
                    buf.write_u8(Self::IPV4)?;
                    buf.write_all(&addr_v4.ip().octets())?;
                    buf.write_u16(addr_v4.port())?;
                }
                SocketAddr::V6(addr_v6) => {
                    buf.write_u8(Self::IPV6)?;
                    buf.write_all(&addr_v6.ip().octets())?;
                    buf.write_u16(addr_v6.port())?;
                }
            }
        }
        Ok(())
    }

    fn read_from(reader: &mut impl ReadInteger) -> Result<Self, InvalidTokenError> {
        let len = reader.read_u32()?;

        if !(1..=MAX_SERVERS_PER_CONNECT as u32).contains(&len) {
            return Err(InvalidTokenError::AddressListLength(len));
        }

        let mut addrs = FreeList::new();

        for _ in 0..len {
            let addr_type = reader.read_u8()?;
            let addr = match addr_type {
                Self::IPV4 => {
                    let mut octets = [0; 4];
                    reader.read_exact(&mut octets)?;
                    let port = reader.read_u16()?;
                    SocketAddr::from((Ipv4Addr::from(octets), port))
                }
                Self::IPV6 => {
                    let mut octets = [0; 16];
                    reader.read_exact(&mut octets)?;
                    let port = reader.read_u16()?;
                    SocketAddr::from((Ipv6Addr::from(octets), port))
                }
                t => return Err(InvalidTokenError::InvalidIpAddressType(t)),
            };
            addrs.insert(addr);
        }

        Ok(Self { addrs })
    }
}

pub struct ConnectTokenPrivate {
    pub client_id: u64,
    pub timeout_seconds: i32,
    pub server_addresses: AddressList,
    pub client_to_server_key: Key,
    pub server_to_client_key: Key,
    pub user_data: [u8; USER_DATA_BYTES],
}

impl ConnectTokenPrivate {
    fn aead(
        protocol_id: u64,
        expire_timestamp: u64,
    ) -> Result<[u8; NETCODE_VERSION.len() + core::mem::size_of::<u64>() * 2], Error> {
        let mut aead = [0; NETCODE_VERSION.len() + core::mem::size_of::<u64>() * 2];
        let mut cursor = io::Cursor::new(&mut aead[..]);
        cursor.write_all(NETCODE_VERSION)?;
        cursor.write_u64(protocol_id)?;
        cursor.write_u64(expire_timestamp)?;
        Ok(aead)
    }

    pub fn encrypt(
        &self,
        protocol_id: u64,
        expire_timestamp: u64,
        nonce: XNonce,
        private_key: &Key,
    ) -> Result<[u8; Self::SIZE], Error> {
        let aead = Self::aead(protocol_id, expire_timestamp)?;
        let mut buf = [0u8; Self::SIZE]; // NOTE: token buffer needs 16-bytes overhead for auth tag
        let mut cursor = io::Cursor::new(&mut buf[..]);
        self.write_to(&mut cursor)?;
        crypto::xchacha_encrypt(&mut buf, Some(&aead), nonce, private_key)?;
        Ok(buf)
    }

    pub fn decrypt(
        encrypted: &mut [u8],
        protocol_id: u64,
        expire_timestamp: u64,
        nonce: XNonce,
        private_key: &Key,
    ) -> Result<Self, Error> {
        let aead = Self::aead(protocol_id, expire_timestamp)?;
        crypto::xchacha_decrypt(encrypted, Some(&aead), nonce, private_key)?;
        let mut cursor = io::Cursor::new(encrypted);
        Ok(Self::read_from(&mut cursor)?)
    }
}

impl Bytes for ConnectTokenPrivate {
    const SIZE: usize = 1024;
    // always padded to 1024 bytes
    type Error = io::Error;
    fn write_to(&self, buf: &mut impl WriteInteger) -> Result<(), io::Error> {
        buf.write_u64(self.client_id)?;
        buf.write_i32(self.timeout_seconds)?;
        self.server_addresses
            .write_to(buf)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
        buf.write_all(&self.client_to_server_key)?;
        buf.write_all(&self.server_to_client_key)?;
        buf.write_all(&self.user_data)?;
        Ok(())
    }

    fn read_from(reader: &mut impl ReadInteger) -> Result<Self, io::Error> {
        let client_id = reader.read_u64()?;
        let timeout_seconds = reader.read_i32()?;
        let server_addresses =
            AddressList::read_from(reader).map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

        let mut client_to_server_key = [0; PRIVATE_KEY_BYTES];
        reader.read_exact(&mut client_to_server_key)?;

        let mut server_to_client_key = [0; PRIVATE_KEY_BYTES];
        reader.read_exact(&mut server_to_client_key)?;

        let mut user_data = [0; USER_DATA_BYTES];
        reader.read_exact(&mut user_data)?;

        Ok(Self {
            client_id,
            timeout_seconds,
            server_addresses,
            client_to_server_key,
            server_to_client_key,
            user_data,
        })
    }
}

pub struct ChallengeToken {
    pub client_id: u64,
    pub user_data: [u8; USER_DATA_BYTES],
}

impl ChallengeToken {
    pub const SIZE: usize = 300;
    pub fn encrypt(&self, sequence: u64, private_key: &Key) -> Result<[u8; Self::SIZE], Error> {
        let mut buf = [0u8; Self::SIZE]; // NOTE: token buffer needs 16-bytes overhead for auth tag
        let mut cursor = io::Cursor::new(&mut buf[..]);
        self.write_to(&mut cursor)?;
        crypto::chacha_encrypt(&mut buf, None, sequence, private_key)?;
        Ok(buf)
    }

    pub fn decrypt(
        encrypted: &mut [u8; Self::SIZE],
        sequence: u64,
        private_key: &Key,
    ) -> Result<Self, Error> {
        crypto::chacha_decrypt(encrypted, None, sequence, private_key)?;
        let mut cursor = io::Cursor::new(&encrypted[..]);
        Ok(Self::read_from(&mut cursor)?)
    }
}

impl Bytes for ChallengeToken {
    const SIZE: usize = size_of::<u64>() + USER_DATA_BYTES;
    type Error = io::Error;
    fn write_to(&self, buf: &mut impl WriteInteger) -> Result<(), io::Error> {
        buf.write_u64(self.client_id)?;
        buf.write_all(&self.user_data)?;
        Ok(())
    }

    fn read_from(reader: &mut impl ReadInteger) -> Result<Self, io::Error> {
        let client_id = reader.read_u64()?;
        let mut user_data = [0; USER_DATA_BYTES];
        reader.read_exact(&mut user_data)?;
        Ok(Self {
            client_id,
            user_data,
        })
    }
}

/// A token containing all the information required for a client to connect to a server.
///
/// The token should be provided to the client by some out-of-band method, such as a web service or a game server browser. <br>
/// See netcode's upstream [specification](https://github.com/networkprotocol/netcode/blob/master/STANDARD.md) for more details.
///
/// # Example
/// ```
/// use crate::lightyear::connection::netcode::{generate_key, ConnectToken, USER_DATA_BYTES, CONNECT_TOKEN_BYTES};
///
/// // mandatory fields
/// let server_address = "192.168.0.0:12345"; // the server's public address (can also be multiple addresses)
/// let private_key = generate_key(); // 32-byte private key, used to encrypt the token
/// let protocol_id = 0x11223344; // must match the server's protocol id - unique to your app/game
/// let client_id = 123; // globally unique identifier for an authenticated client
///
/// // optional fields
/// let expire_seconds = -1; // defaults to 30 seconds, negative for no expiry
/// let timeout_seconds = -1; // defaults to 15 seconds, negative for no timeout
/// let user_data = [0u8; USER_DATA_BYTES]; // custom data
///
/// let connect_token = ConnectToken::build(server_address, protocol_id, client_id, private_key)
///     .expire_seconds(expire_seconds)
///     .timeout_seconds(timeout_seconds)
///     .user_data(user_data)
///     .generate()
///     .unwrap();
///
/// // Serialize the connect token to a 2048-byte array
/// let token_bytes = connect_token.try_into_bytes().unwrap();
/// assert_eq!(token_bytes.len(), CONNECT_TOKEN_BYTES);
/// ```
///
/// Alternatively, you can use [`Server::token`](crate::connection::netcode::server::NetcodeServer::token) to generate a connect token from an already existing [`Server`](crate::connection::netcode::server::NetcodeServer).
#[derive(Clone)]
pub struct ConnectToken {
    pub(crate) version_info: [u8; NETCODE_VERSION.len()],
    pub(crate) protocol_id: u64,
    pub(crate) create_timestamp: u64,
    pub(crate) expire_timestamp: u64,
    pub(crate) nonce: XNonce,
    pub(crate) private_data: [u8; ConnectTokenPrivate::SIZE],
    pub(crate) timeout_seconds: i32,
    pub(crate) server_addresses: AddressList,
    pub(crate) client_to_server_key: Key,
    pub(crate) server_to_client_key: Key,
}

/// A builder that can be used to generate a connect token.
pub struct ConnectTokenBuilder<A: utils::ToSocketAddrs> {
    protocol_id: u64,
    client_id: u64,
    expire_seconds: i32,
    private_key: Key,
    timeout_seconds: i32,
    public_server_addresses: A,
    internal_server_addresses: Option<AddressList>,
    user_data: [u8; USER_DATA_BYTES],
}

impl<A: utils::ToSocketAddrs> ConnectTokenBuilder<A> {
    fn new(server_addresses: A, protocol_id: u64, client_id: u64, private_key: Key) -> Self {
        Self {
            protocol_id,
            client_id,
            expire_seconds: TOKEN_EXPIRE_SEC,
            private_key,
            timeout_seconds: CONNECTION_TIMEOUT_SEC,
            public_server_addresses: server_addresses,
            internal_server_addresses: None,
            user_data: [0; USER_DATA_BYTES],
        }
    }
    /// Sets the time in seconds that the token will be valid for.
    ///
    /// Negative values will disable expiry.
    pub fn expire_seconds(mut self, expire_seconds: i32) -> Self {
        self.expire_seconds = expire_seconds;
        self
    }
    /// Sets the time in seconds that a connection will be kept alive without any packets being received.
    ///
    /// Negative values will disable timeouts.
    pub fn timeout_seconds(mut self, timeout_seconds: i32) -> Self {
        self.timeout_seconds = timeout_seconds;
        self
    }
    /// Sets the user data that will be added to the token, this can be any data you want.
    pub fn user_data(mut self, user_data: [u8; USER_DATA_BYTES]) -> Self {
        self.user_data = user_data;
        self
    }
    /// Sets the **internal** server addresses in the private data of the token. <br>
    /// If this field is not set, the **public** server addresses provided when creating the builder will be used instead.
    ///
    /// The **internal** server addresses list is used by the server to determine if the client is connecting to the same server that issued the token.
    /// The client will always use the **public** server addresses list to connect to the server, never the **internal** ones.
    ///
    /// This is useful for when you bind your server to a local address that is not accessible from the internet,
    /// but you want to provide a public address that is accessible to the client.
    pub fn internal_addresses(mut self, internal_addresses: A) -> Result<Self, Error> {
        self.internal_server_addresses = Some(AddressList::new(internal_addresses)?);
        Ok(self)
    }
    /// Generates the token and consumes the builder.
    pub fn generate(self) -> Result<ConnectToken, Error> {
        // number of seconds since unix epoch
        let now = utils::now()?;
        let expire_timestamp = if self.expire_seconds < 0 {
            u64::MAX
        } else {
            now + self.expire_seconds as u64
        };
        let public_server_addresses = AddressList::new(self.public_server_addresses)?;
        let internal_server_addresses = match self.internal_server_addresses {
            Some(addresses) => addresses,
            None => public_server_addresses,
        };
        let client_to_server_key = crypto::generate_key();
        let server_to_client_key = crypto::generate_key();
        let nonce = XChaCha20Poly1305::generate_nonce(&mut OsRng);

        let private_data = ConnectTokenPrivate {
            client_id: self.client_id,
            timeout_seconds: self.timeout_seconds,
            server_addresses: internal_server_addresses,
            client_to_server_key,
            server_to_client_key,
            user_data: self.user_data,
        }
        .encrypt(self.protocol_id, expire_timestamp, nonce, &self.private_key)?;

        Ok(ConnectToken {
            version_info: *NETCODE_VERSION,
            protocol_id: self.protocol_id,
            create_timestamp: now,
            expire_timestamp,
            nonce,
            private_data,
            timeout_seconds: self.timeout_seconds,
            server_addresses: public_server_addresses,
            client_to_server_key,
            server_to_client_key,
        })
    }
}

impl ConnectToken {
    /// Creates a new connect token builder that can be used to generate a connect token.
    pub fn build<A: utils::ToSocketAddrs>(
        server_addresses: A,
        protocol_id: u64,
        client_id: u64,
        private_key: Key,
    ) -> ConnectTokenBuilder<A> {
        ConnectTokenBuilder::new(server_addresses, protocol_id, client_id, private_key)
    }

    /// Tries to convert the token into a 2048-byte array.
    pub fn try_into_bytes(self) -> Result<[u8; CONNECT_TOKEN_BYTES], io::Error> {
        let mut buf = [0u8; CONNECT_TOKEN_BYTES];
        let mut cursor = io::Cursor::new(&mut buf[..]);
        self.write_to(&mut cursor).map_err(|e| {
            io::Error::new(
                io::ErrorKind::Other,
                format!("failed to write token to buffer: {}", e).as_str(),
            )
        })?;
        Ok(buf)
    }

    /// Tries to convert a 2048-byte array into a connect token.
    pub fn try_from_bytes(bytes: &[u8]) -> Result<Self, InvalidTokenError> {
        let mut cursor = io::Cursor::new(bytes);
        Self::read_from(&mut cursor)
    }
}

impl Bytes for ConnectToken {
    const SIZE: usize = 2048;
    // always padded to 2048 bytes
    type Error = InvalidTokenError;
    fn write_to(&self, buf: &mut impl WriteInteger) -> Result<(), Self::Error> {
        buf.write_all(&self.version_info)?;
        buf.write_u64(self.protocol_id)?;
        buf.write_u64(self.create_timestamp)?;
        buf.write_u64(self.expire_timestamp)?;
        buf.write_all(&self.nonce)?;
        buf.write_all(&self.private_data)?;
        buf.write_i32(self.timeout_seconds)?;
        self.server_addresses.write_to(buf)?;
        buf.write_all(&self.client_to_server_key)?;
        buf.write_all(&self.server_to_client_key)?;
        Ok(())
    }

    fn read_from(reader: &mut impl ReadInteger) -> Result<Self, Self::Error> {
        let mut version_info = [0; NETCODE_VERSION.len()];
        reader.read_exact(&mut version_info)?;

        if version_info != *NETCODE_VERSION {
            return Err(InvalidTokenError::InvalidVersion);
        }

        let protocol_id = reader.read_u64()?;

        let create_timestamp = reader.read_u64()?;
        let expire_timestamp = reader.read_u64()?;

        if create_timestamp > expire_timestamp {
            return Err(InvalidTokenError::InvalidTimestamp);
        }

        let mut nonce = [0; size_of::<XNonce>()];
        reader.read_exact(&mut nonce)?;
        let nonce = XNonce::from_slice(&nonce).to_owned();

        let mut private_data = [0; ConnectTokenPrivate::SIZE];
        reader.read_exact(&mut private_data)?;

        let timeout_seconds = reader.read_i32()?;

        let server_addresses = AddressList::read_from(reader)?;

        let mut client_to_server_key = [0; PRIVATE_KEY_BYTES];
        reader.read_exact(&mut client_to_server_key)?;

        let mut server_to_client_key = [0; PRIVATE_KEY_BYTES];
        reader.read_exact(&mut server_to_client_key)?;

        Ok(Self {
            version_info,
            protocol_id,
            create_timestamp,
            expire_timestamp,
            nonce,
            private_data,
            timeout_seconds,
            server_addresses,
            client_to_server_key,
            server_to_client_key,
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::connection::netcode::utils::ToSocketAddrs;
    use super::*;
    #[cfg(not(feature = "std"))]
    use alloc::vec::Vec;

    #[test]
    fn encrypt_decrypt_private_token() {
        let private_key = crypto::generate_key();
        let protocol_id = 1;
        let expire_timestamp = 2;
        let nonce = XChaCha20Poly1305::generate_nonce(&mut OsRng);
        let client_id = 4;
        let timeout_seconds = 5;
        let server_addresses = AddressList::new(
            &[
                SocketAddr::from(([127, 0, 0, 1], 1)),
                SocketAddr::from(([127, 0, 0, 1], 2)),
                SocketAddr::from(([127, 0, 0, 1], 3)),
                SocketAddr::from(([127, 0, 0, 1], 4)),
            ][..],
        )
        .unwrap();
        let user_data = [0x11; USER_DATA_BYTES];

        let private_token = ConnectTokenPrivate {
            client_id,
            timeout_seconds,
            server_addresses,
            user_data,
            client_to_server_key: crypto::generate_key(),
            server_to_client_key: crypto::generate_key(),
        };

        let mut encrypted = private_token
            .encrypt(protocol_id, expire_timestamp, nonce, &private_key)
            .unwrap();

        let private_token = ConnectTokenPrivate::decrypt(
            &mut encrypted,
            protocol_id,
            expire_timestamp,
            nonce,
            &private_key,
        )
        .unwrap();

        assert_eq!(private_token.client_id, client_id);
        assert_eq!(private_token.timeout_seconds, timeout_seconds);
        private_token
            .server_addresses
            .iter()
            .zip(server_addresses.iter())
            .for_each(|(have, expected)| {
                assert_eq!(have, expected);
            });
        assert_eq!(private_token.user_data, user_data);
        assert_eq!(
            private_token.server_to_client_key,
            private_token.server_to_client_key
        );
        assert_eq!(
            private_token.client_to_server_key,
            private_token.client_to_server_key
        );
    }

    #[test]
    fn encrypt_decrypt_challenge_token() {
        let private_key = crypto::generate_key();
        let sequence = 1;
        let client_id = 2;
        let user_data = [0x11; USER_DATA_BYTES];

        let challenge_token = ChallengeToken {
            client_id,
            user_data,
        };

        let mut encrypted = challenge_token.encrypt(sequence, &private_key).unwrap();

        let challenge_token =
            ChallengeToken::decrypt(&mut encrypted, sequence, &private_key).unwrap();

        assert_eq!(challenge_token.client_id, client_id);
        assert_eq!(challenge_token.user_data, user_data);
    }

    #[test]
    fn connect_token_read_write() {
        let private_key = crypto::generate_key();
        let protocol_id = 1;
        let expire_timestamp = 2;
        let nonce = XChaCha20Poly1305::generate_nonce(&mut OsRng);
        let client_id = 4;
        let timeout_seconds = 5;
        let server_addresses = AddressList::new(
            &[
                SocketAddr::from(([127, 0, 0, 1], 1)),
                SocketAddr::from(([127, 0, 0, 1], 2)),
                SocketAddr::from(([127, 0, 0, 1], 3)),
                SocketAddr::from(([127, 0, 0, 1], 4)),
            ][..],
        )
        .unwrap();
        let user_data = [0x11; USER_DATA_BYTES];

        let private_token = ConnectTokenPrivate {
            client_id,
            timeout_seconds,
            server_addresses,
            user_data,
            client_to_server_key: crypto::generate_key(),
            server_to_client_key: crypto::generate_key(),
        };

        let mut encrypted = private_token
            .encrypt(protocol_id, expire_timestamp, nonce, &private_key)
            .unwrap();

        let private_token = ConnectTokenPrivate::decrypt(
            &mut encrypted,
            protocol_id,
            expire_timestamp,
            nonce,
            &private_key,
        )
        .unwrap();

        let mut private_data = [0; ConnectTokenPrivate::SIZE];
        let mut cursor = io::Cursor::new(&mut private_data[..]);
        private_token.write_to(&mut cursor).unwrap();

        let connect_token = ConnectToken {
            version_info: *NETCODE_VERSION,
            protocol_id,
            create_timestamp: 0,
            expire_timestamp,
            nonce,
            private_data,
            timeout_seconds,
            server_addresses,
            client_to_server_key: private_token.client_to_server_key,
            server_to_client_key: private_token.server_to_client_key,
        };

        let mut buf = Vec::new();
        connect_token.write_to(&mut buf).unwrap();

        let connect_token = ConnectToken::read_from(&mut buf.as_slice()).unwrap();

        assert_eq!(connect_token.version_info, *NETCODE_VERSION);
        assert_eq!(connect_token.protocol_id, protocol_id);
        assert_eq!(connect_token.create_timestamp, 0);
        assert_eq!(connect_token.expire_timestamp, expire_timestamp);
        assert_eq!(connect_token.nonce, nonce);
        assert_eq!(connect_token.private_data, private_data);
        assert_eq!(connect_token.timeout_seconds, timeout_seconds);
        connect_token
            .server_addresses
            .iter()
            .zip(server_addresses.iter())
            .for_each(|(have, expected)| {
                assert_eq!(have, expected);
            });
    }

    #[test]
    fn connect_token_builder() {
        let protocol_id = 1;
        let client_id = 4;
        let server_addresses = "127.0.0.1:12345";

        let connect_token = ConnectToken::build(
            server_addresses,
            protocol_id,
            client_id,
            [0x42; PRIVATE_KEY_BYTES],
        )
        .user_data([0x11; USER_DATA_BYTES])
        .timeout_seconds(5)
        .expire_seconds(6)
        .internal_addresses("0.0.0.0:0")
        .expect("failed to parse address")
        .generate()
        .unwrap();

        assert_eq!(connect_token.version_info, *NETCODE_VERSION);
        assert_eq!(connect_token.protocol_id, protocol_id);
        assert_eq!(connect_token.timeout_seconds, 5);
        assert_eq!(
            connect_token.expire_timestamp,
            connect_token.create_timestamp + 6
        );
        connect_token
            .server_addresses
            .iter()
            .zip(server_addresses.to_socket_addrs().into_iter().flatten())
            .for_each(|((_, have), expected)| {
                assert_eq!(have, expected);
            });
    }
}
