//! Common run conditions
use crate::connection::server::ServerConnections;
use crate::prelude::server::ServerConfig;
use crate::prelude::{Mode, NetworkIdentity};
use bevy::prelude::{Ref, Res};

/// Returns true if the peer is a client
pub fn is_client(identity: NetworkIdentity) -> bool {
    identity.is_client()
}

/// Returns true if the peer is a server
pub fn is_server(identity: NetworkIdentity) -> bool {
    identity.is_server()
}

/// Returns true if we are running in host-server mode, i.e. the server is acting as a client
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

pub fn is_host_server_ref(
    config: Option<Ref<ServerConfig>>,
    server: Option<Ref<ServerConnections>>,
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

// /// Returns true if we are ready to buffer the server replication messages
// pub fn is_server_replication_send_ready(
//     timer: Option<Res<SendIntervalTimer<server::ConnectionManager>>>,
// ) -> bool {
//     timer.map_or(false, |t| t.timer.as_ref().map_or(true, |t| t.finished()))
// }
//
// /// Returns true if we are ready to buffer the client replication messages
// pub fn is_client_replication_send_ready(
//     timer: Option<Res<SendIntervalTimer<client::ConnectionManager>>>,
// ) -> bool {
//     timer.map_or(false, |t| t.timer.as_ref().map_or(true, |t| t.finished()))
// }
