use super::{
    CONNECT_TOKEN_BYTES, CONNECTION_TIMEOUT_SEC, IDENTITY_TOKEN_HEADER_BYTES,
    IDENTITY_TOKEN_VERSION, IDENTITY_USER_DATA_BYTES, IdentityBinding, NETCODE_VERSION,
    PRIVATE_KEY_BYTES, USER_DATA_BYTES,
    bytes::Bytes,
    crypto::{self, Key},
    error::Error,
    utils,
};
use alloc::borrow::ToOwned;
use alloc::format;
use chacha20poly1305::{AeadCore, XChaCha20Poly1305, XNonce, aead::OsRng};
use core::mem::size_of;
use core::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use lightyear_serde::reader::ReadInteger;
use lightyear_serde::writer::WriteInteger;
use no_std_io2::io::{self, Write};
use thiserror::Error;

const MAX_SERVERS_PER_CONNECT: usize = 32;
pub(crate) const TOKEN_EXPIRE_SEC: i32 = 30;

/// An error that can occur when de-serializing a connect token from bytes.
#[derive(Error, Debug)]
pub enum InvalidTokenError {
    #[error("address list length is out of range 0-32: {0}")]
    AddressListLength(u32),
    #[error("socket tokens require nonempty public and private address lists")]
    EmptySocketAddressList,
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
    addrs: [SocketAddr; MAX_SERVERS_PER_CONNECT],
    len: usize,
}

impl AddressList {
    const IPV4: u8 = 1;
    const IPV6: u8 = 2;
    fn empty() -> Self {
        Self {
            addrs: [SocketAddr::from(([0, 0, 0, 0], 0)); MAX_SERVERS_PER_CONNECT],
            len: 0,
        }
    }
    pub fn new(addrs: impl utils::ToSocketAddrs) -> Result<Self, Error> {
        let mut addresses = Self::empty();
        for addr in addrs.to_socket_addrs()?.take(MAX_SERVERS_PER_CONNECT) {
            addresses.addrs[addresses.len] = addr;
            addresses.len += 1;
        }
        Ok(addresses)
    }
    fn nonempty(addrs: impl utils::ToSocketAddrs) -> Result<Self, Error> {
        let addresses = Self::new(addrs)?;
        if addresses.len == 0 {
            return Err(Error::InvalidToken(
                InvalidTokenError::EmptySocketAddressList,
            ));
        }
        Ok(addresses)
    }
    pub fn len(&self) -> usize {
        self.len
    }
    pub fn as_slice(&self) -> &[SocketAddr] {
        &self.addrs[..self.len]
    }
    pub fn iter(&self) -> impl Iterator<Item = (usize, SocketAddr)> + '_ {
        self.as_slice().iter().copied().enumerate()
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

        if len > MAX_SERVERS_PER_CONNECT as u32 {
            return Err(InvalidTokenError::AddressListLength(len));
        }

        let mut addresses = Self::empty();

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
            addresses.addrs[addresses.len] = addr;
            addresses.len += 1;
        }

        Ok(addresses)
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
            .map_err(io::Error::other)?;
        buf.write_all(&self.client_to_server_key)?;
        buf.write_all(&self.server_to_client_key)?;
        buf.write_all(&self.user_data)?;
        Ok(())
    }

    fn read_from(reader: &mut impl ReadInteger) -> Result<Self, io::Error> {
        let client_id = reader.read_u64()?;
        let timeout_seconds = reader.read_i32()?;
        let server_addresses = AddressList::read_from(reader).map_err(io::Error::other)?;

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
/// use lightyear_netcode::{generate_key, ConnectToken, CONNECT_TOKEN_BYTES, USER_DATA_BYTES};
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
/// Alternatively, you can use [`Server::token`](crate::server::Server::token) to generate a connect token from an already existing [`Server`](crate::server::Server).
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

impl ConnectToken {
    /// The timestamp when the token expires, in seconds since the unix epoch.
    pub fn expire_timestamp(&self) -> u64 {
        self.expire_timestamp
    }
}

/// A builder for socket-bound tokens, with all 256 user-data bytes available to applications.
pub struct ConnectTokenBuilder<A: utils::ToSocketAddrs> {
    settings: TokenBuilder,
    public_server_addresses: A,
    internal_server_addresses: Option<AddressList>,
}

impl<A: utils::ToSocketAddrs> ConnectTokenBuilder<A> {
    fn new(server_addresses: A, protocol_id: u64, client_id: u64, private_key: Key) -> Self {
        Self {
            settings: TokenBuilder::new(protocol_id, client_id, private_key),
            public_server_addresses: server_addresses,
            internal_server_addresses: None,
        }
    }

    /// Sets the time in seconds that the token will be valid for.
    ///
    /// Negative values will disable expiry.
    pub fn expire_seconds(mut self, expire_seconds: i32) -> Self {
        self.settings.expire_seconds = expire_seconds;
        self
    }
    /// Sets the time in seconds that a connection will be kept alive without any packets being received.
    ///
    /// Negative values will disable timeouts.
    pub fn timeout_seconds(mut self, timeout_seconds: i32) -> Self {
        self.settings.timeout_seconds = timeout_seconds;
        self
    }
    /// Sets unrestricted application data. No prefix selects identity authorization.
    pub fn user_data(mut self, user_data: [u8; USER_DATA_BYTES]) -> Self {
        self.settings.user_data = user_data;
        self
    }
    /// Sets the nonempty **internal** server addresses in the private data of the token. <br>
    /// If this field is not set, the **public** server addresses provided when creating the builder will be used instead.
    ///
    /// The server compares these **internal** addresses against its explicitly configured
    /// expected authorization addresses. The client exposes the **public** addresses for
    /// application-managed routing; netcode does not select or configure the transport.
    ///
    /// This is useful when the server authorizes an internal socket address but clients
    /// dial a public socket address, hostname, or URL.
    pub fn internal_addresses(mut self, internal_addresses: A) -> Result<Self, Error> {
        self.internal_server_addresses = Some(AddressList::nonempty(internal_addresses)?);
        Ok(self)
    }
    /// Generates the token and consumes the builder.
    ///
    /// Both address lists must be nonempty. Client ID zero is reserved for the server
    /// role and cannot be minted; all other IDs, including `u64::MAX`, are allowed.
    pub fn generate(self) -> Result<ConnectToken, Error> {
        if self.settings.client_id == 0 {
            return Err(Error::ReservedClientId);
        }
        let public_addresses = AddressList::nonempty(self.public_server_addresses)?;
        let private_addresses = self.internal_server_addresses.unwrap_or(public_addresses);
        self.settings.generate(public_addresses, private_addresses)
    }
}

/// A builder for identity-bound tokens with no public or private socket addresses.
///
/// The authenticated user-data region reserves 33 bytes for the version and exact
/// server binding, leaving [`IDENTITY_USER_DATA_BYTES`] (223) application bytes.
/// Native transport routing and authentication remain the caller's responsibility.
pub struct IdentityConnectTokenBuilder {
    settings: TokenBuilder,
}

impl IdentityConnectTokenBuilder {
    fn new(binding: IdentityBinding, protocol_id: u64, client_id: u64, private_key: Key) -> Self {
        let mut settings = TokenBuilder::new(protocol_id, client_id, private_key);
        settings.user_data[0] = IDENTITY_TOKEN_VERSION;
        settings.user_data[1..IDENTITY_TOKEN_HEADER_BYTES].copy_from_slice(binding.as_bytes());
        Self { settings }
    }

    /// Sets the validity duration in seconds; negative values disable expiry.
    pub fn expire_seconds(mut self, expire_seconds: i32) -> Self {
        self.settings.expire_seconds = expire_seconds;
        self
    }

    /// Sets the connection timeout in seconds; negative values disable timeouts.
    pub fn timeout_seconds(mut self, timeout_seconds: i32) -> Self {
        self.settings.timeout_seconds = timeout_seconds;
        self
    }

    /// Sets the 223 application bytes after the authenticated identity header.
    pub fn user_data(mut self, user_data: [u8; IDENTITY_USER_DATA_BYTES]) -> Self {
        self.settings.user_data[IDENTITY_TOKEN_HEADER_BYTES..].copy_from_slice(&user_data);
        self
    }

    /// Generates the token. Client ID zero is reserved; `u64::MAX` is allowed.
    pub fn generate(self) -> Result<ConnectToken, Error> {
        self.settings
            .generate(AddressList::empty(), AddressList::empty())
    }
}

struct TokenBuilder {
    protocol_id: u64,
    client_id: u64,
    expire_seconds: i32,
    private_key: Key,
    timeout_seconds: i32,
    user_data: [u8; USER_DATA_BYTES],
}

impl TokenBuilder {
    fn new(protocol_id: u64, client_id: u64, private_key: Key) -> Self {
        Self {
            protocol_id,
            client_id,
            expire_seconds: TOKEN_EXPIRE_SEC,
            private_key,
            timeout_seconds: CONNECTION_TIMEOUT_SEC,
            user_data: [0; USER_DATA_BYTES],
        }
    }

    fn generate(
        self,
        public_server_addresses: AddressList,
        internal_server_addresses: AddressList,
    ) -> Result<ConnectToken, Error> {
        if self.client_id == 0 {
            return Err(Error::ReservedClientId);
        }
        // number of seconds since unix epoch
        let now = utils::now()?;
        let expire_timestamp = if self.expire_seconds < 0 {
            u64::MAX
        } else {
            now + self.expire_seconds as u64
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

    /// Creates an identity-bound token builder with empty address lists.
    ///
    /// The server must explicitly select identity authorization with this exact binding.
    /// The client independently authenticates the native identity when dialing.
    pub fn build_identity(
        binding: IdentityBinding,
        protocol_id: u64,
        client_id: u64,
        private_key: Key,
    ) -> IdentityConnectTokenBuilder {
        IdentityConnectTokenBuilder::new(binding, protocol_id, client_id, private_key)
    }

    /// Tries to convert the token into a 2048-byte array.
    pub fn try_into_bytes(self) -> Result<[u8; CONNECT_TOKEN_BYTES], io::Error> {
        let mut buf = [0u8; CONNECT_TOKEN_BYTES];
        let mut cursor = io::Cursor::new(&mut buf[..]);
        self.write_to(&mut cursor)
            .map_err(|e| io::Error::other(format!("failed to write token to buffer: {e}")))?;
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
        #[allow(deprecated)]
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
    use super::*;

    use alloc::vec::Vec;
    use sha2::{Digest, Sha256};

    fn vectors() -> serde_json::Value {
        serde_json::from_str(include_str!("token_vectors.json")).unwrap()
    }

    fn hex(value: &serde_json::Value) -> Vec<u8> {
        let value = value.as_str().unwrap();
        assert_eq!(value.len() % 2, 0);
        (0..value.len())
            .step_by(2)
            .map(|index| u8::from_str_radix(&value[index..index + 2], 16).unwrap())
            .collect()
    }

    fn hex_u64(value: &serde_json::Value) -> u64 {
        u64::from_str_radix(value.as_str().unwrap().strip_prefix("0x").unwrap(), 16).unwrap()
    }

    fn decrypt(token: &ConnectToken, key: &Key) -> ConnectTokenPrivate {
        let mut private_data = token.private_data;
        ConnectTokenPrivate::decrypt(
            &mut private_data,
            token.protocol_id,
            token.expire_timestamp,
            token.nonce,
            key,
        )
        .unwrap()
    }

    #[test]
    fn independent_token_vectors() {
        // Generated with Python struct + hashlib + PyNaCl, not this codec.
        // Both modes deliberately carry identical, identity-looking raw user data.
        let fixture = vectors();
        let private_key: Key = hex(&fixture["private_key_hex"]).try_into().unwrap();
        let user_data: [u8; USER_DATA_BYTES] = hex(&fixture["user_data_hex"]).try_into().unwrap();
        let binding = IdentityBinding::from_native(
            fixture["identity_domain"].as_str().unwrap().as_bytes(),
            &hex(&fixture["native_identity_hex"]),
        );
        assert_eq!(
            binding.as_bytes().as_slice(),
            hex(&fixture["identity_binding_hex"])
        );
        assert_eq!(
            Sha256::digest(hex(&fixture["binding_preimage_hex"])).as_slice(),
            binding.as_bytes()
        );
        assert_eq!(
            user_data[0],
            fixture["identity_version"].as_u64().unwrap() as u8
        );
        assert_eq!(&user_data[1..33], binding.as_bytes());
        assert_eq!(
            IDENTITY_TOKEN_HEADER_BYTES,
            fixture["identity_application_data_offset"]
                .as_u64()
                .unwrap() as usize
        );
        assert_eq!(USER_DATA_BYTES, 256);
        assert_eq!(IDENTITY_USER_DATA_BYTES, 223);

        for vector in fixture["vectors"].as_array().unwrap() {
            let bytes = hex(&vector["token_hex"]);
            assert_eq!(bytes.len(), 2048);
            let token = ConnectToken::try_from_bytes(&bytes).unwrap();
            let expected_addresses: Vec<SocketAddr> = vector["addresses"]
                .as_array()
                .unwrap()
                .iter()
                .map(|address| address.as_str().unwrap().parse().unwrap())
                .collect();
            assert_eq!(token.protocol_id, hex_u64(&fixture["protocol_id_hex"]));
            assert_eq!(
                token.create_timestamp,
                fixture["create_timestamp"].as_u64().unwrap()
            );
            assert_eq!(
                token.expire_timestamp,
                fixture["expire_timestamp"].as_u64().unwrap()
            );
            assert_eq!(
                token.timeout_seconds,
                fixture["timeout_seconds"].as_i64().unwrap() as i32
            );
            assert_eq!(token.nonce.as_slice(), hex(&fixture["nonce_hex"]));
            assert_eq!(token.server_addresses.as_slice(), expected_addresses);
            assert_eq!(
                token.client_to_server_key.as_slice(),
                hex(&fixture["client_to_server_key_hex"])
            );
            assert_eq!(
                token.server_to_client_key.as_slice(),
                hex(&fixture["server_to_client_key_hex"])
            );

            let private = decrypt(&token, &private_key);
            assert_eq!(private.client_id, hex_u64(&fixture["client_id_hex"]));
            assert_eq!(private.timeout_seconds, token.timeout_seconds);
            assert_eq!(private.server_addresses.as_slice(), expected_addresses);
            assert_eq!(private.client_to_server_key, token.client_to_server_key);
            assert_eq!(private.server_to_client_key, token.server_to_client_key);
            assert_eq!(private.user_data, user_data);
            match vector["mode"].as_str().unwrap() {
                "socket" => assert_eq!(private.server_addresses.len(), 1),
                "identity" => {
                    assert_eq!(private.server_addresses.len(), 0);
                    assert_eq!(private.user_data[0], IDENTITY_TOKEN_VERSION);
                    assert_eq!(&private.user_data[1..33], binding.as_bytes());
                    assert_eq!(&private.user_data[33..], &[0x33; 223]);
                }
                mode => panic!("unexpected fixture mode: {mode}"),
            }

            let mut plaintext = [0; ConnectTokenPrivate::SIZE - 16];
            private
                .write_to(&mut io::Cursor::new(&mut plaintext[..]))
                .unwrap();
            assert_eq!(
                Sha256::digest(plaintext).as_slice(),
                hex(&vector["private_plaintext_sha256"])
            );
            assert_eq!(
                private
                    .encrypt(
                        token.protocol_id,
                        token.expire_timestamp,
                        token.nonce,
                        &private_key
                    )
                    .unwrap(),
                token.private_data
            );
            assert_eq!(token.try_into_bytes().unwrap().as_slice(), bytes);
        }
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
    fn address_list_structural_boundaries() {
        let mut empty_bytes = Vec::new();
        AddressList::empty().write_to(&mut empty_bytes).unwrap();
        assert_eq!(empty_bytes, [0, 0, 0, 0]);
        assert_eq!(
            AddressList::read_from(&mut empty_bytes.as_slice())
                .unwrap()
                .len(),
            0
        );

        // Maximum-size IPv6 lists must fit and retain every address and big-endian port.
        let addresses: Vec<SocketAddr> = (0..32)
            .map(|index| SocketAddr::from((Ipv6Addr::LOCALHOST, 0x1200 + index)))
            .collect();
        let mut encoded = Vec::new();
        AddressList::new(addresses.as_slice())
            .unwrap()
            .write_to(&mut encoded)
            .unwrap();
        assert_eq!(encoded.len(), AddressList::SIZE);
        assert_eq!(&encoded[..5], &[0, 0, 0, 32, 2]);
        assert_eq!(&encoded[21..23], &[0x12, 0]);
        assert_eq!(
            AddressList::read_from(&mut encoded.as_slice())
                .unwrap()
                .as_slice(),
            addresses
        );
        for end in 0..encoded.len() {
            assert!(
                matches!(
                    AddressList::read_from(&mut &encoded[..end]),
                    Err(InvalidTokenError::Io(_))
                ),
                "accepted truncated address list at {end}"
            );
        }
        for count in [33u32, u32::MAX] {
            assert!(matches!(
                AddressList::read_from(&mut count.to_be_bytes().as_slice()),
                Err(InvalidTokenError::AddressListLength(value)) if value == count
            ));
        }
        assert!(matches!(
            AddressList::read_from(&mut [0, 0, 0, 1, 3].as_slice()),
            Err(InvalidTokenError::InvalidIpAddressType(3))
        ));
    }

    #[test]
    fn token_decoders_reject_malformed_address_lists() {
        let fixture = vectors();
        let socket_bytes = hex(&fixture["vectors"][0]["token_hex"]);
        let private_key: Key = hex(&fixture["private_key_hex"]).try_into().unwrap();
        let token = ConnectToken::try_from_bytes(&socket_bytes).unwrap();
        let private = decrypt(&token, &private_key);
        // Offsets follow the unchanged envelope/private field order, not user data.
        let public_count_offset = NETCODE_VERSION.len() + 8 * 3 + 24 + 1024 + 4;
        let private_count_offset = 8 + 4;
        for count in [33u32, u32::MAX] {
            let mut bytes = socket_bytes.clone();
            bytes[public_count_offset..public_count_offset + 4]
                .copy_from_slice(&count.to_be_bytes());
            assert!(matches!(
                ConnectToken::try_from_bytes(&bytes),
                Err(InvalidTokenError::AddressListLength(value)) if value == count
            ));
            let mut encrypted = [0; ConnectTokenPrivate::SIZE];
            private
                .write_to(&mut io::Cursor::new(&mut encrypted[..]))
                .unwrap();
            encrypted[private_count_offset..private_count_offset + 4]
                .copy_from_slice(&count.to_be_bytes());
            let aead =
                ConnectTokenPrivate::aead(token.protocol_id, token.expire_timestamp).unwrap();
            crypto::xchacha_encrypt(&mut encrypted, Some(&aead), token.nonce, &private_key)
                .unwrap();
            assert!(
                ConnectTokenPrivate::decrypt(
                    &mut encrypted,
                    token.protocol_id,
                    token.expire_timestamp,
                    token.nonce,
                    &private_key
                )
                .is_err()
            );
        }
        let mut private_bytes = Vec::new();
        private.write_to(&mut private_bytes).unwrap();
        // Every missing field byte must fail; envelope padding is not a field.
        let public_fields_end = public_count_offset + 4 + 7 + PRIVATE_KEY_BYTES * 2;
        for end in 0..public_fields_end {
            assert!(ConnectToken::try_from_bytes(&socket_bytes[..end]).is_err());
        }
        for end in 0..private_bytes.len() {
            assert!(ConnectTokenPrivate::read_from(&mut &private_bytes[..end]).is_err());
        }
    }

    #[test]
    fn token_builder_rejects_reserved_client_id() {
        assert!(matches!(
            ConnectToken::build("127.0.0.1:5000", 1, 0, [0; PRIVATE_KEY_BYTES]).generate(),
            Err(Error::ReservedClientId)
        ));
        assert!(matches!(
            ConnectToken::build_identity(
                IdentityBinding::from_bytes([0; 32]),
                1,
                0,
                [0; PRIVATE_KEY_BYTES]
            )
            .generate(),
            Err(Error::ReservedClientId)
        ));
    }

    #[test]
    fn socket_builder_rejects_empty_address_lists() {
        let empty: &[SocketAddr] = &[];
        let addresses = [SocketAddr::from(([127, 0, 0, 1], 5000))];
        assert!(matches!(
            ConnectToken::build(empty, 1, 1, [0; PRIVATE_KEY_BYTES]).generate(),
            Err(Error::InvalidToken(
                InvalidTokenError::EmptySocketAddressList
            ))
        ));
        assert!(matches!(
            ConnectToken::build(addresses.as_slice(), 1, 1, [0; PRIVATE_KEY_BYTES])
                .internal_addresses(empty),
            Err(Error::InvalidToken(
                InvalidTokenError::EmptySocketAddressList
            ))
        ));
        assert!(matches!(
            ConnectToken::build(empty, 1, 1, [0; PRIVATE_KEY_BYTES])
                .internal_addresses(addresses.as_slice())
                .unwrap()
                .generate(),
            Err(Error::InvalidToken(
                InvalidTokenError::EmptySocketAddressList
            ))
        ));
    }

    #[test]
    fn token_builders_preserve_mode_and_user_data_capacity() {
        let fixture = vectors();
        let raw_data: [u8; USER_DATA_BYTES] = hex(&fixture["user_data_hex"]).try_into().unwrap();
        let binding =
            IdentityBinding::from_bytes(hex(&fixture["identity_binding_hex"]).try_into().unwrap());
        let private_key = [0x42; PRIVATE_KEY_BYTES];
        let public_address = SocketAddr::from(([127, 0, 0, 1], 5000));
        let internal_address = SocketAddr::from(([10, 0, 0, 1], 5001));
        let socket = ConnectToken::build(public_address, 1, u64::MAX, private_key)
            .internal_addresses(internal_address)
            .unwrap()
            .user_data(raw_data)
            .timeout_seconds(5)
            .expire_seconds(6)
            .generate()
            .unwrap();
        let socket = ConnectToken::try_from_bytes(&socket.try_into_bytes().unwrap()).unwrap();
        let private = decrypt(&socket, &private_key);
        assert_eq!(socket.server_addresses.as_slice(), &[public_address]);
        assert_eq!(private.server_addresses.as_slice(), &[internal_address]);
        assert_eq!(private.client_id, u64::MAX);
        assert_eq!(private.user_data, raw_data);
        assert_eq!(private.timeout_seconds, 5);
        assert_eq!(socket.timeout_seconds, 5);
        assert_eq!(socket.expire_timestamp, socket.create_timestamp + 6);

        let application_data: [u8; IDENTITY_USER_DATA_BYTES] =
            core::array::from_fn(|index| index as u8);
        let identity = ConnectToken::build_identity(binding, 1, u64::MAX, private_key)
            .user_data(application_data)
            .timeout_seconds(-1)
            .expire_seconds(-1)
            .generate()
            .unwrap();
        let identity = ConnectToken::try_from_bytes(&identity.try_into_bytes().unwrap()).unwrap();
        let private = decrypt(&identity, &private_key);
        assert!(identity.server_addresses.as_slice().is_empty());
        assert!(private.server_addresses.as_slice().is_empty());
        assert_eq!(private.client_id, u64::MAX);
        assert_eq!(private.user_data[0], IDENTITY_TOKEN_VERSION);
        assert_eq!(&private.user_data[1..33], binding.as_bytes());
        assert_eq!(&private.user_data[33..], &application_data);
        assert_eq!(private.timeout_seconds, -1);
        assert_eq!(identity.timeout_seconds, -1);
        assert_eq!(identity.expire_timestamp, u64::MAX);
    }
}
