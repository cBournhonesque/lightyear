//! Module to handle the various possible ClientIds
use crate::serialize::reader::Reader;
use crate::serialize::{SerializationError, ToBytes};
use bevy::reflect::Reflect;
use byteorder::{NetworkEndian, ReadBytesExt, WriteBytesExt};
use serde::{Deserialize, Serialize};
use std::fmt::Formatter;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Reflect)]
pub enum ClientId {
    /// A client id that is unique between netcode connections
    Netcode(u64),
    /// The client id of a steam user
    Steam(u64),
    /// A local client to use when running in HostServer mode
    Local(u64),
    ///
    Server,
}

impl ToBytes for ClientId {
    fn len(&self) -> usize {
        9
    }

    fn to_bytes<T: WriteBytesExt>(&self, buffer: &mut T) -> Result<(), SerializationError> {
        match self {
            ClientId::Netcode(id) => {
                buffer.write_u8(0)?;
                buffer.write_u64::<NetworkEndian>(*id)?;
            }
            ClientId::Steam(id) => {
                buffer.write_u8(1)?;
                buffer.write_u64::<NetworkEndian>(*id)?;
            }
            ClientId::Local(id) => {
                buffer.write_u8(2)?;
                buffer.write_u64::<NetworkEndian>(*id)?;
            }
            ClientId::Server => {
                buffer.write_u8(3)?;
            }
        }
        Ok(())
    }

    fn from_bytes(buffer: &mut Reader) -> Result<Self, SerializationError>
    where
        Self: Sized,
    {
        match buffer.read_u8()? {
            0 => Ok(ClientId::Netcode(buffer.read_u64::<NetworkEndian>()?)),
            1 => Ok(ClientId::Steam(buffer.read_u64::<NetworkEndian>()?)),
            2 => Ok(ClientId::Local(buffer.read_u64::<NetworkEndian>()?)),
            3 => Ok(ClientId::Server),
            _ => Err(SerializationError::InvalidValue),
        }
    }
}

impl ClientId {
    // TODO: add impl From<ClientId> for u64?
    /// Convert a ClientId to a u64 representation
    pub fn to_bits(&self) -> u64 {
        match self {
            ClientId::Netcode(x) => *x,
            ClientId::Steam(x) => *x,
            ClientId::Local(x) => *x,
            ClientId::Server => 0,
        }
    }

    pub fn is_local(&self) -> bool {
        matches!(self, ClientId::Local(_))
    }
}

impl core::fmt::Display for ClientId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        core::fmt::Debug::fmt(self, f)
        // match self {
        //     ClientId::Netcode(id) => write!(f, "NetcodeClientId({})", id),
        //     ClientId::Steam(id) => write!(f, "SteamClientId({})", id),
        //     ClientId::LocalClient => write!(f, "LocalClientId"),
        // }
    }
}
