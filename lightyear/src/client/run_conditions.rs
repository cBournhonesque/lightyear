//! Common client-related run conditions
use bevy::prelude::Res;

use crate::{
    client::connection::ConnectionManager,
    connection::client::{ClientConnection, ConnectionState, NetClient},
};

/// Returns true if the client is connected
///
/// We check the status of the ClientConnection directly instead of using the `State<NetworkingState>`
/// to avoid having a frame of delay since the `StateTransition` schedule runs after `PreUpdate`.
/// We also check both the networking state and the io state (in case the io gets disconnected)
pub fn is_connected(netclient: Option<Res<ClientConnection>>) -> bool {
    netclient.is_some_and(|c| matches!(c.state(), ConnectionState::Connected))
}

/// Returns true if the client is disconnected.
///
/// We check the status of the ClientConnection directly instead of using the `State<NetworkingState>`
/// to avoid having a frame of delay since the `StateTransition` schedule runs after `PreUpdate`
pub fn is_disconnected(netclient: Option<Res<ClientConnection>>) -> bool {
    netclient.as_ref().map_or(true, |c| {
        matches!(c.state(), ConnectionState::Disconnected { .. })
    })
}

/// Run condition if the client is connected and synced (i.e. the client tick is synced with the server)
pub fn is_synced(
    netclient: Option<Res<ClientConnection>>,
    connection: Option<Res<ConnectionManager>>,
) -> bool {
    netclient.is_some_and(|c| matches!(c.state(), ConnectionState::Connected)) &&
        // TODO: check if this correct; in host-server mode, the client is always synced
        connection.is_some_and(|c| c.sync_manager.is_synced())
}
