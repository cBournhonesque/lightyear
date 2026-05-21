//! Shared connection request policy types.
//!
//! These types are protocol-neutral. Concrete server implementations can use
//! [`crate::shared::ConnectionRequestHandler`] to decide whether a remote
//! [`lightyear_core::id::PeerId`] should be accepted before the rest of the connection lifecycle
//! becomes [`crate::client::Connected`].

use alloc::string::String;
use core::fmt::Debug;
use lightyear_core::id::PeerId;
use serde::{Deserialize, Serialize};

/// Reason a server rejected a connection request.
#[derive(Debug, PartialEq, Clone, Serialize, Deserialize)]
pub enum DeniedReason {
    /// The server has no remaining capacity for this client.
    ServerFull,
    /// The peer or account is banned.
    Banned,
    /// The server hit an internal error while processing the request.
    InternalError,
    /// The same peer is already connected.
    AlreadyConnected,
    /// The authentication token was already consumed.
    TokenAlreadyUsed,
    /// The authentication token was malformed, expired, or otherwise invalid.
    InvalidToken,
    /// Application-specific denial reason.
    Custom(String),
}

/// Trait for handling connection requests from clients.
pub trait ConnectionRequestHandler: Debug + Send + Sync {
    /// Handles a connection request from `client_id`.
    ///
    /// Return `None` to accept the request, or `Some(reason)` to deny it.
    fn handle_request(&self, client_id: PeerId) -> Option<DeniedReason>;
}

/// By default, all connection requests are accepted by the server.
#[derive(Debug, Clone)]
pub struct DefaultConnectionRequestHandler;

impl ConnectionRequestHandler for DefaultConnectionRequestHandler {
    fn handle_request(&self, _client_id: PeerId) -> Option<DeniedReason> {
        None
    }
}
