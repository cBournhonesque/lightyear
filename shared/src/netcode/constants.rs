use std::time::Duration;

pub(crate) const NETCODE_VERSION_LEN: usize = 13;
pub(crate) const NETCODE_VERSION_INFO: &[u8; NETCODE_VERSION_LEN] = b"NETCODE 1.02\0";
pub(crate) const NETCODE_MAX_CLIENTS: usize = 1024;
pub(crate) const NETCODE_MAX_PENDING_CLIENTS: usize = NETCODE_MAX_CLIENTS * 4;

pub(crate) const NETCODE_ADDRESS_NONE: u8 = 0;
pub(crate) const NETCODE_ADDRESS_IPV4: u8 = 1;
pub(crate) const NETCODE_ADDRESS_IPV6: u8 = 2;

pub(crate) const NETCODE_CONNECT_TOKEN_PRIVATE_BYTES: usize = 1024;
/// The maximum number of bytes that a netcode packet can contain.
pub(crate) const NETCODE_MAX_PACKET_BYTES: usize = 1400;
/// The maximum number of bytes that a payload can have when generating a payload packet.
pub(crate) const NETCODE_MAX_PAYLOAD_BYTES: usize = 1300;

/// The number of bytes in a private key;
pub(crate) const NETCODE_KEY_BYTES: usize = 32;
pub(crate) const NETCODE_MAC_BYTES: usize = 16;
/// The number of bytes that an user data can contain in the ConnectToken.
pub(crate) const NETCODE_USER_DATA_BYTES: usize = 256;
pub(crate) const NETCODE_CHALLENGE_TOKEN_BYTES: usize = 300;
pub(crate) const NETCODE_CONNECT_TOKEN_XNONCE_BYTES: usize = 24;

pub(crate) const NETCODE_ADDITIONAL_DATA_SIZE: usize = NETCODE_VERSION_LEN + 8 + 8;

pub(crate) const NETCODE_TIMEOUT_SECONDS: i32 = 15;

pub(crate) const NETCODE_SEND_RATE: Duration = Duration::from_millis(250);
