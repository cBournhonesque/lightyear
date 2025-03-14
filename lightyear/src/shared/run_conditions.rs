//! Common run conditions
use crate::shared::identity::NetworkIdentityState;
use bevy::prelude::{Ref, Res, State};

/// Returns true if the peer is a client (host-server counts as a server)
pub fn is_client(identity: Option<Res<State<NetworkIdentityState>>>) -> bool {
    identity.is_some_and(|i| i.get() == &NetworkIdentityState::Client)
}

/// Returns true if the peer is a server
pub fn is_server(identity: Option<Res<State<NetworkIdentityState>>>) -> bool {
    identity.is_some_and(|i| i.get() != &NetworkIdentityState::Client)
}

/// Returns true if we are running in host-server mode, i.e. the server is acting as a client
/// (in which case we can disable the networking/prediction/interpolation systems on the client)
///
/// We are in HostServer mode if the mode is set to HostServer AND the server is running.
/// (checking if the mode is set to HostServer is not enough, it just means that the server plugin
/// and client plugin are running in the same App)
pub fn is_host_server(identity: Option<Res<State<NetworkIdentityState>>>) -> bool {
    identity.is_some_and(|i| i.get() == &NetworkIdentityState::HostServer)
}


/// Returns true if the peer is a client (host-server counts as a server)
pub(crate) fn is_client_ref(identity: Option<Ref<State<NetworkIdentityState>>>) -> bool {
    identity.is_some_and(|i| i.get() == &NetworkIdentityState::Client)
}

pub(crate) fn is_server_ref(identity: Option<Ref<State<NetworkIdentityState>>>) -> bool {
    identity.is_some_and(|i| i.get() != &NetworkIdentityState::Client)
}

pub(crate) fn is_host_server_ref(identity: Option<Ref<State<NetworkIdentityState>>>) -> bool {
    identity.is_some_and(|i| i.get() == &NetworkIdentityState::HostServer)
}