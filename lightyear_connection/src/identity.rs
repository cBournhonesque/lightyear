use crate::client::Client;
use crate::host::HostClient;
use bevy::prelude::*;
use lightyear_link::server::Server;

/// Returns true if the peer is a client (host-server counts as a server)
pub fn is_client(query: Query<(), (With<Client>, Without<HostClient>)>) -> bool {
    !query.is_empty()
}

/// Returns true if the peer is a server
pub fn is_server(query: Query<(), With<Server>>) -> bool {
    !query.is_empty()
}


/// Returns true if we are running in host-server mode, i.e. the server is acting as a client
/// (in which case we can disable the networking/prediction/interpolation systems on the client)
///
/// We are in HostServer mode if the mode is set to HostServer AND the server is running.
/// (checking if the mode is set to HostServer is not enough, it just means that the server plugin
/// and client plugin are running in the same App)
pub fn is_host_server() -> bool {
    todo!();
    // identity.is_some_and(|i| i.get() == &NetworkIdentityState::HostServer)
}