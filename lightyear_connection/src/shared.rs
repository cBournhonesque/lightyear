#[cfg(not(feature = "std"))]
use alloc::string::String;
use core::fmt::Debug;
use lightyear_core::id::PeerId;
use serde::{Deserialize, Serialize};

/// Reasons for denying a connection request
#[derive(Debug, PartialEq, Clone, Serialize, Deserialize)]
pub enum DeniedReason {
    ServerFull,
    Banned,
    InternalError,
    AlreadyConnected,
    TokenAlreadyUsed,
    InvalidToken,
    Custom(String),
}

/// Trait for handling connection requests from clients.
pub trait ConnectionRequestHandler: Debug + Send + Sync {
    /// Handle a connection request from a client.
    /// Returns None if the connection is accepted,
    /// Returns Some(reason) if the connection is denied.
    fn handle_request(&self, client_id: PeerId) -> Option<DeniedReason>;
}

/// By default, all connection requests are accepted by the server.
#[derive(Debug, Clone)]
pub struct DefaultConnectionRequestHandler;

impl ConnectionRequestHandler for DefaultConnectionRequestHandler {
    fn handle_request(&self, client_id: PeerId) -> Option<DeniedReason> {
        None
    }
}