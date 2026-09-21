//! Opaque routing aliases and explicitly specified native identity projections.
use core::fmt::{Display, Formatter};
use core::net::SocketAddr;
use lightyear_serde::reader::{ReadInteger, Reader};
use lightyear_serde::writer::WriteInteger;
use lightyear_serde::{SerializationError, ToBytes};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use bevy_derive::Deref;
use bevy_ecs::component::Component;
use bevy_reflect::Reflect;

/// Stores the PeerId of the local peer for the connection
#[derive(
    Debug, Component, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Reflect, Deref,
)]
pub struct LocalId(pub PeerId);

impl Display for LocalId {
    fn fmt(&self, f: &mut Formatter<'_>) -> core::fmt::Result {
        core::fmt::Debug::fmt(&self.0, f)
    }
}

/// Stores the PeerId of the remote peer that we are connected to
#[derive(
    Debug, Component, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Reflect, Deref,
)]
#[component(immutable)]
pub struct RemoteId(pub PeerId);

impl Display for RemoteId {
    fn fmt(&self, f: &mut Formatter<'_>) -> core::fmt::Result {
        core::fmt::Debug::fmt(&self.0, f)
    }
}

/// An opaque, World-wide routing alias, not an account or transport address.
///
/// Zero is reserved for the client/server server sentinel. Ordinary peers use `1..=u64::MAX`.
/// Native numeric IDs are preserved; larger native identities require an explicit projection.
/// Ordering is numeric, so every member of a P2P cohort assigns the same deterministic slots.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Reflect,
)]
#[serde(transparent)]
pub struct PeerId(pub u64);

impl ToBytes for PeerId {
    fn bytes_len(&self) -> usize {
        8
    }

    fn to_bytes(&self, buffer: &mut impl WriteInteger) -> Result<(), SerializationError> {
        buffer.write_u64(self.0)?;
        Ok(())
    }

    fn from_bytes(buffer: &mut Reader) -> Result<Self, SerializationError> {
        Ok(Self(buffer.read_u64()?))
    }
}

impl PeerId {
    /// The server side of an explicitly client/server connection, never an ordinary peer.
    pub const SERVER: Self = Self(0);
}

impl Display for PeerId {
    fn fmt(&self, f: &mut Formatter<'_>) -> core::fmt::Result {
        Display::fmt(&self.0, f)
    }
}

/// Projects canonical native identity bytes into a nonzero routing alias.
///
/// SHA-256 input is `b"lightyear-peer-id-v1\0"`, the domain byte length as a big-endian u64,
/// the domain, then the canonical identity bytes. Take the first eight digest bytes as a
/// big-endian u64, mapping zero to one. Domains are protocol constants, not Rust type names.
/// This is not authentication; retain the full native identity and reject shared-ID collisions.
pub fn peer_id_from_native(domain: &[u8], identity: &[u8]) -> PeerId {
    let mut hash = Sha256::new();
    hash.update(b"lightyear-peer-id-v1\0");
    hash.update((domain.len() as u64).to_be_bytes());
    hash.update(domain);
    hash.update(identity);
    let digest = hash.finalize();
    PeerId(u64::from_be_bytes(core::array::from_fn(|i| digest[i])).max(1))
}

/// Projects a socket routing identity without embedding its address in [`PeerId`].
///
/// Canonical bytes: family byte 4 or 6, network-order IP octets, big-endian port, and for
/// IPv6 a big-endian u32 scope ID. IPv4-mapped IPv6 normalizes to IPv4; flow labels are excluded.
/// Built-in transport domains are `b"udp"`, `b"websocket"`, and `b"webtransport"`.
pub fn peer_id_from_socket(domain: &[u8], address: SocketAddr) -> PeerId {
    let mut bytes = [0u8; 23];
    let len = match address.ip().to_canonical() {
        core::net::IpAddr::V4(ip) => {
            bytes[0] = 4;
            bytes[1..5].copy_from_slice(&ip.octets());
            bytes[5..7].copy_from_slice(&address.port().to_be_bytes());
            7
        }
        core::net::IpAddr::V6(ip) => {
            bytes[0] = 6;
            bytes[1..17].copy_from_slice(&ip.octets());
            bytes[17..19].copy_from_slice(&address.port().to_be_bytes());
            if let SocketAddr::V6(address) = address {
                bytes[19..23].copy_from_slice(&address.scope_id().to_be_bytes());
            }
            23
        }
    };
    peer_id_from_native(domain, &bytes[..len])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routing_wire_is_fixed_big_endian_without_a_variant_tag() {
        let id = PeerId(0x0102_0304_0506_0708);
        let mut bytes = alloc::vec::Vec::new();
        id.to_bytes(&mut bytes).unwrap();
        assert_eq!(bytes, [1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(PeerId::from_bytes(&mut Reader::from(bytes)).unwrap(), id);
        assert_eq!(
            PeerId::from_bytes(&mut Reader::from(alloc::vec![255; 8])).unwrap(),
            PeerId(u64::MAX)
        );
        assert_eq!(
            PeerId::from_bytes(&mut Reader::from(alloc::vec![0; 8])).unwrap(),
            PeerId::SERVER
        );
    }

    #[test]
    fn socket_projection_has_canonical_domain_separated_vectors() {
        let address: SocketAddr = "127.0.0.1:4100".parse().unwrap();
        let expected = PeerId(0xd1c7_6592_148b_73b6);
        assert_eq!(peer_id_from_socket(b"udp", address), expected);
        assert_eq!(
            peer_id_from_socket(b"udp", "[::ffff:127.0.0.1]:4100".parse().unwrap()),
            expected
        );
        assert_ne!(peer_id_from_socket(b"websocket", address), expected);
        let ip = "fe80::1".parse().unwrap();
        let scoped = SocketAddr::V6(core::net::SocketAddrV6::new(ip, 4100, 0, 3));
        let changed_flow = SocketAddr::V6(core::net::SocketAddrV6::new(ip, 4100, 42, 3));
        let changed_scope = SocketAddr::V6(core::net::SocketAddrV6::new(ip, 4100, 0, 4));
        assert_eq!(
            peer_id_from_socket(b"udp", scoped),
            peer_id_from_socket(b"udp", changed_flow)
        );
        assert_ne!(
            peer_id_from_socket(b"udp", scoped),
            peer_id_from_socket(b"udp", changed_scope)
        );
    }
}
