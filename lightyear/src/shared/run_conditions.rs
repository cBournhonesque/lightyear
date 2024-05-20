//! Common run conditions

use crate::client::networking::NetworkingState;
use crate::connection::client::{ClientConnection, NetClient};
use crate::connection::server::ServerConnections;
use crate::prelude::server::ServerConfig;
use crate::prelude::{Mode, NetworkIdentity};
use crate::transport::io::IoState;
use bevy::prelude::Res;

/// Returns true if the peer is a client
pub fn is_client(identity: NetworkIdentity) -> bool {
    identity.is_client()
}

/// Returns true if the peer is a server
pub fn is_server(identity: NetworkIdentity) -> bool {
    identity.is_server()
}

/// Returns true if we are running the server, but the server is acting as a client.
/// (in which case we can disable the networking/prediction/interpolation systems on the client)
///
/// We are in HostServer mode if the mode is set to HostServer AND the server is running.
/// (checking if the mode is set to HostServer is not enough, it just means that the server plugin
/// and client plugin are running in the same App)
pub fn is_host_server(
    config: Option<Res<ServerConfig>>,
    server: Option<Res<ServerConnections>>,
) -> bool {
    config.map_or(false, |config| {
        matches!(config.shared.mode, Mode::HostServer)
            && server.map_or(false, |server| server.is_listening())
    })
}

/// Returns true if the `SharedConfig` is set to `Mode::Separate`
/// (i.e. we are not running in HostServer mode)
pub fn is_mode_separate(config: Option<Res<ServerConfig>>) -> bool {
    config.map_or(true, |config| config.shared.mode == Mode::Separate)
}

/// Returns true if the client is connected
///
/// We check the status of the ClientConnection directly instead of using the `State<NetworkingState>`
/// to avoid having a frame of delay since the `StateTransition` schedule runs after `PreUpdate`.
/// We also check both the networking state and the io state (in case the io gets disconnected)
pub(crate) fn is_connected(netclient: Option<Res<ClientConnection>>) -> bool {
    netclient.map_or(false, |c| {
        c.state() == NetworkingState::Connected
            && c.io()
                // TODO: maybe we don't need to check io because an io disconnect will trigger
                //  a netcode disconnect?
                // we default to true for connections that don't use io
                .map_or(true, |io| matches!(io.state, IoState::Connected))
    })
}

/// Returns true if the client is disconnected.
///
/// We check the status of the ClientConnection directly instead of using the `State<NetworkingState>`
/// to avoid having a frame of delay since the `StateTransition` schedule runs after `PreUpdate`
pub(crate) fn is_disconnected(netclient: Option<Res<ClientConnection>>) -> bool {
    netclient.as_ref().map_or(true, |c| {
        c.state() == NetworkingState::Disconnected
            || c.io()
                .map_or(true, |io| !matches!(io.state, IoState::Connected))
    })
}

/// Returns true if the server is started.
///
/// We check the status of the `ServerConnections` directly instead of using the `State<NetworkingState>`
/// to avoid having a frame of delay since the `StateTransition` schedule runs after the `PreUpdate` schedule
pub(crate) fn is_started(server: Option<Res<ServerConnections>>) -> bool {
    server.map_or(false, |s| s.is_listening())
}

/// Returns true if the server is stopped.
///
/// We check the status of the `ServerConnections` directly instead of using the `State<NetworkingState>`
/// to avoid having a frame of delay since the `StateTransition` schedule runs after the `PreUpdate` schedule
pub(crate) fn is_stopped(server: Option<Res<ServerConnections>>) -> bool {
    server.map_or(true, |s| !s.is_listening())
}
