use crate::client::Client;
use crate::host::{HostClient, HostServer};
use crate::server::{Started, Starting};
use bevy_ecs::query::{Or, With, Without};
use bevy_ecs::system::Query;
use lightyear_link::server::Server;

/// Returns true if the peer is a client (host-server counts as a server)
pub fn is_client(query: Query<(), (With<Client>, Without<HostClient>)>) -> bool {
    !query.is_empty()
}

/// Returns true if the peer is starting or running a server.
pub fn is_server(query: Query<(), (With<Server>, Or<(With<Starting>, With<Started>)>)>) -> bool {
    !query.is_empty()
}

/// Returns true if the peer is starting or running a server without any client entities.
pub fn is_headless_server(
    server_query: Query<(), (With<Server>, Or<(With<Starting>, With<Started>)>)>,
    client_query: Query<(), With<Client>>,
) -> bool {
    !server_query.is_empty() && client_query.is_empty()
}

/// Returns true if we are running in host-server mode, i.e. the server is acting as a client
/// (in which case we can disable the networking/prediction/interpolation systems on the client)
///
/// We are in host-server mode when a running server has been marked as a host server.
pub fn is_host_server(query: Query<(), (With<Server>, With<Started>, With<HostServer>)>) -> bool {
    !query.is_empty()
}
