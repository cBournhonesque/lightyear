//! Module to handle the various possible ClientIds
use bevy::prelude::{Component, Deref};
use bevy::reflect::Reflect;
use core::fmt::Formatter;
use lightyear_serde::reader::{ReadInteger, Reader};
use lightyear_serde::writer::WriteInteger;
use lightyear_serde::{SerializationError, ToBytes};
use serde::{Deserialize, Serialize};

/// Stores the PeerId of the local peer for the connection
#[derive(
    Debug, Component, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Reflect, Deref,
)]
pub struct LocalId(pub PeerId);

/// Stores the PeerId of the remote peer that we are connected to
#[derive(
    Debug, Component, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Reflect, Deref,
)]
pub struct RemoteId(pub PeerId);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Reflect)]
pub enum PeerId {
    Entity(u64),
    // #[reflect(ignore)]
    // // #[reflect(default = "LOCALHOST")]
    // IP(SocketAddr),
    /// A client id that is unique between netcode connections
    Netcode(u64),
    /// The client id of a steam user
    Steam(u64),
    /// A local client to use when running in HostServer mode
    Local(u64),
    /// Refers to the server
    Server,
}

impl ToBytes for PeerId {
    fn bytes_len(&self) -> usize {
        match self {
            PeerId::Entity(_) => 1 + 8,
            // PeerId::IP(socket_addr) => {
            //     // 1 byte for variant + address bytes + 2 bytes for port
            //     match socket_addr {
            //         SocketAddr::V4(_) => 1 + 4 + 2,
            //         SocketAddr::V6(_) => 1 + 16 + 2,
            //     }
            // },
            PeerId::Netcode(_) => 1 + 8,
            PeerId::Steam(_) => 1 + 8,
            PeerId::Local(_) => 1 + 8,
            PeerId::Server => 1,
        }
    }

    fn to_bytes(&self, buffer: &mut impl WriteInteger) -> Result<(), SerializationError> {
        match self {
            PeerId::Entity(id) => {
                buffer.write_u8(0)?;
                buffer.write_u64(*id)?;
            }
            // PeerId::IP(socket) => {
            //     match socket {
            //         SocketAddr::V4(v4) => {
            //             buffer.write_u8(1)?;
            //             // Write IPv4 bytes
            //             for byte in v4.ip().octets().iter() {
            //                 buffer.write_u8(*byte)?;
            //             }
            //             // Write port
            //             buffer.write_u16(v4.port())?;
            //         }
            //         SocketAddr::V6(v6) => {
            //             buffer.write_u8(2)?;
            //             // Write IPv6 bytes
            //             for segment in v6.ip().segments().iter() {
            //                 buffer.write_u16(*segment)?;
            //             }
            //             // Write port
            //             buffer.write_u16(v6.port())?;
            //         }
            //     }
            // },
            PeerId::Netcode(id) => {
                buffer.write_u8(3)?;
                buffer.write_u64(*id)?;
            }
            PeerId::Steam(id) => {
                buffer.write_u8(4)?;
                buffer.write_u64(*id)?;
            }
            PeerId::Local(id) => {
                buffer.write_u8(5)?;
                buffer.write_u64(*id)?;
            }
            PeerId::Server => {
                buffer.write_u8(6)?;
            }
        }
        Ok(())
    }

    fn from_bytes(buffer: &mut Reader) -> Result<Self, SerializationError>
    where
        Self: Sized,
    {
        match buffer.read_u8()? {
            0 => Ok(PeerId::Entity(buffer.read_u64()?)),
            // 1 => {
            //     // Read IPv4 address
            //     let a = buffer.read_u8()?;
            //     let b = buffer.read_u8()?;
            //     let c = buffer.read_u8()?;
            //     let d = buffer.read_u8()?;
            //     let port = buffer.read_u16()?;
            //     let addr = SocketAddr::new(
            //         std::net::IpAddr::V4(std::net::Ipv4Addr::new(a, b, c, d)),
            //         port
            //     );
            //     Ok(PeerId::IP(addr))
            // },
            // 2 => {
            //     // Read IPv6 address
            //     let mut segments = [0u16; 8];
            //     for i in 0..8 {
            //         segments[i] = buffer.read_u16()?;
            //     }
            //     let port = buffer.read_u16()?;
            //     let addr = SocketAddr::new(
            //         std::net::IpAddr::V6(std::net::Ipv6Addr::from(segments)),
            //         port
            //     );
            //     Ok(PeerId::IP(addr))
            // },
            3 => Ok(PeerId::Netcode(buffer.read_u64()?)),
            4 => Ok(PeerId::Steam(buffer.read_u64()?)),
            5 => Ok(PeerId::Local(buffer.read_u64()?)),
            6 => Ok(PeerId::Server),
            _ => Err(SerializationError::InvalidValue),
        }
    }
}

impl PeerId {
    // TODO: add impl From<ClientId> for u64?
    /// Convert a ClientId to a u64 representation
    pub fn to_bits(&self) -> u64 {
        match self {
            PeerId::Entity(x) => *x,
            // PeerId::IP(socket) => {
            //     // Create a simple hash for the IP address
            //     // Just for differentiation - not meant to be a secure hash
            //     match socket {
            //         SocketAddr::V4(v4) => {
            //             let octets = v4.ip().octets();
            //             let mut result: u64 = 0x0200; // Prefix for IPv4
            //             result |= (octets[0] as u64) << 24;
            //             result |= (octets[1] as u64) << 16;
            //             result |= (octets[2] as u64) << 8;
            //             result |= octets[3] as u64;
            //             result |= (v4.port() as u64) << 32;
            //             result
            //         },
            //         SocketAddr::V6(v6) => {
            //             // Simple hash for IPv6 - fold the segments
            //             let segments = v6.ip().segments();
            //             let mut hash: u64 = 0x0600; // Prefix for IPv6
            //             for (i, &segment) in segments.iter().enumerate() {
            //                 hash ^= (segment as u64) << ((i % 4) * 16);
            //             }
            //             hash ^= (v6.port() as u64) << 48;
            //             hash
            //         }
            //     }
            // },
            PeerId::Netcode(x) => *x,
            PeerId::Steam(x) => *x,
            PeerId::Local(x) => *x,
            PeerId::Server => 1,
        }
    }

    pub fn is_local(&self) -> bool {
        matches!(self, PeerId::Local(_))
    }
}

impl core::fmt::Display for PeerId {
    fn fmt(&self, f: &mut Formatter<'_>) -> core::fmt::Result {
        core::fmt::Debug::fmt(self, f)
        // match self {
        //     ClientId::Netcode(id) => write!(f, "NetcodeClientId({})", id),
        //     ClientId::Steam(id) => write!(f, "SteamClientId({})", id),
        //     ClientId::LocalClient => write!(f, "LocalClientId"),
        // }
    }
}
