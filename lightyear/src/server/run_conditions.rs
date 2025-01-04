//! Common server-related run conditions
use crate::connection::server::ServerConnections;
use crate::prelude::server::NetworkingState;
use bevy::prelude::{Res, State, World};

/// Returns true if the server is started.
///
/// We check the status of the `ServerConnections` directly instead of using the `State<NetworkingState>`
/// to avoid having a frame of delay since the `StateTransition` schedule runs after the `PreUpdate` schedule
pub fn is_started(server: Option<Res<State<NetworkingState>>>) -> bool {
    server.map_or(false, |s| s.get() != &NetworkingState::Stopped)
}

/// Returns true if the server is stopped.
///
/// We check the status of the `ServerConnections` directly instead of using the `State<NetworkingState>`
/// to avoid having a frame of delay since the `StateTransition` schedule runs after the `PreUpdate` schedule
pub fn is_stopped(server: Option<Res<State<NetworkingState>>>) -> bool {
    server.map_or(true, |s| s.get() == &NetworkingState::Stopped)
}

pub(crate) trait NetworkingStateExt {
    fn is_started(&self) -> bool;
    fn is_stopped(&self) -> bool;
}

impl NetworkingStateExt for &World {
    fn is_started(&self) -> bool {
        self.get_resource::<State<NetworkingState>>()
            .map_or(false, |s| s.get() != &NetworkingState::Stopped)
    }

    fn is_stopped(&self) -> bool {
        self.get_resource::<State<NetworkingState>>()
            .map_or(true, |s| s.get() == &NetworkingState::Stopped)
    }
}
