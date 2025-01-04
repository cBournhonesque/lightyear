//! Common server-related run conditions
use crate::prelude::server::NetworkingState;
use bevy::prelude::{Ref, Res, State};

/// Returns true if the server is started.
///
/// We check the status of the `ServerConnections` directly instead of using the `State<NetworkingState>`
/// to avoid having a frame of delay since the `StateTransition` schedule runs after the `PreUpdate` schedule
pub fn is_started(server: Option<Res<State<NetworkingState>>>) -> bool {
    server.map_or(false, |s| s.get() == &NetworkingState::Started)
}

/// Returns true if the server is stopped.
///
/// We check the status of the `ServerConnections` directly instead of using the `State<NetworkingState>`
/// to avoid having a frame of delay since the `StateTransition` schedule runs after the `PreUpdate` schedule
pub fn is_stopped(server: Option<Res<State<NetworkingState>>>) -> bool {
    server.map_or(true, |s| s.get() == &NetworkingState::Stopped)
}

pub(crate) fn is_started_ref(server: Option<Ref<State<NetworkingState>>>) -> bool {
    server.map_or(false, |s| s.get() == &NetworkingState::Started)
}
pub(crate) fn is_stopped_ref(server: Option<Ref<State<NetworkingState>>>) -> bool {
    server.map_or(true, |s| s.get() == &NetworkingState::Stopped)
}
