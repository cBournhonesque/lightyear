//! Module to handle the various possible ClientIds

use bevy::reflect::Reflect;
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
}

impl ClientId {
    // TODO: add impl From<ClientId> for u64?
    /// Convert a ClientId to a u64 representation
    pub fn to_bits(&self) -> u64 {
        match self {
            ClientId::Netcode(x) => *x,
            ClientId::Steam(x) => *x,
            ClientId::Local(x) => *x,
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
