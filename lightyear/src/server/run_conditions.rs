//! Common server-related run conditions
use crate::connection::server::ServerConnections;
use bevy::prelude::Res;

/// Returns true if the server is started.
///
/// We check the status of the `ServerConnections` directly instead of using the `State<NetworkingState>`
/// to avoid having a frame of delay since the `StateTransition` schedule runs after the `PreUpdate` schedule
pub fn is_started(server: Option<Res<ServerConnections>>) -> bool {
    server.map_or(false, |s| s.is_listening())
}

/// Returns true if the server is stopped.
///
/// We check the status of the `ServerConnections` directly instead of using the `State<NetworkingState>`
/// to avoid having a frame of delay since the `StateTransition` schedule runs after the `PreUpdate` schedule
pub fn is_stopped(server: Option<Res<ServerConnections>>) -> bool {
    server.map_or(true, |s| !s.is_listening())
}
