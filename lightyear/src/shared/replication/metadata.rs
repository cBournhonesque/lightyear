//! Module that handles the creation of a single Entity per client
//! These can be used to:
//! - track some information about each client
//! - send information to clients about themselves (such as their global ClientId, independent from the connection's ClientId) or about other clients

use crate::prelude::ClientId;
use bevy::prelude::Component;
use lightyear_macros::MessageInternal;
use serde::{Deserialize, Serialize};

#[derive(MessageInternal, Component, Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct ClientMetadata {
    /// global ClientId that is used by the server to identify the client
    pub(crate) client_id: ClientId,
}

// TODO: add another component to specify if the client is the player or another player (or a bot?)
