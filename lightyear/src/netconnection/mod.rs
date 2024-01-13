pub(crate) mod client;

pub(crate) mod server;

pub(crate) mod steam;

pub(crate) mod netcode;

/// The client id from a connect token, must be unique for each client.
///
/// Note that this is not the same as the [`ClientId`], which is used by the server to identify clients.
pub type ClientId = u64;
