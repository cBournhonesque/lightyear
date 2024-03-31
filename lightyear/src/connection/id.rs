//! Module to handle the various possible ClientIds

use serde::{Deserialize, Serialize};
use std::fmt::Formatter;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ClientId {
    /// A client id that is unique between netcode connections
    Netcode(u64),
    /// The client id of a steam user
    Steam(u64),
    /// The local client to use when running in HostServer mode
    LocalClient,
}

impl ClientId {
    /// Convert a ClientId to a u64 representation
    pub fn to_bits(&self) -> u64 {
        match self {
            ClientId::Netcode(x) => *x,
            ClientId::Steam(x) => *x,
            ClientId::LocalClient => 0,
        }
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
