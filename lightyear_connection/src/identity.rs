//! Run conditions for local network identity.
//!
//! These helpers are intentionally small wrappers around ECS queries. They are useful when a system
//! should only run for a pure client, a server, or a future host-server mode.

use crate::client::Client;
use crate::host::HostClient;
use bevy_ecs::query::{With, Without};
use bevy_ecs::system::Query;
use lightyear_link::server::Server;

/// Returns `true` if the local app has a non-host client entity.
///
/// A host-client marked with [`HostClient`] is excluded because it runs inside the same app as the
/// server.
pub fn is_client(query: Query<(), (With<Client>, Without<HostClient>)>) -> bool {
    !query.is_empty()
}

/// Returns `true` if the local app has a server entity.
pub fn is_server(query: Query<(), With<Server>>) -> bool {
    !query.is_empty()
}

/// Returns `true` if the app is running in host-server mode.
///
/// Host-server mode means that the app contains both server logic and a local client connected to
/// that server. This helper is not implemented yet.
/// (in which case we can disable the networking/prediction/interpolation systems on the client)
///
/// We are in HostServer mode if the mode is set to HostServer AND the server is running.
/// (checking if the mode is set to HostServer is not enough, it just means that the server plugin
/// and client plugin are running in the same App)
pub fn is_host_server() -> bool {
    todo!();
    // identity.is_some_and(|i| i.get() == &NetworkIdentityState::HostServer)
}
