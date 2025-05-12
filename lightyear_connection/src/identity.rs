use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

/// Returns true if the peer is a client (host-server counts as a server)
pub fn is_client(identity: Option<Res<NetworkIdentityState>>) -> bool {
    identity.is_some_and(|i| matches!(*i, NetworkIdentityState::Client))
}

/// Returns true if the peer is a server or a host server.
pub fn is_server(identity: Option<Res<NetworkIdentityState>>) -> bool {
    identity.is_some_and(|i| {
        matches!(
            *i,
            NetworkIdentityState::Server | NetworkIdentityState::HostServer
        )
    })
}

/// Returns true if we are running in host-server mode, i.e. the server is acting as a client
/// (in which case we can disable the networking/prediction/interpolation systems on the client)
///
/// We are in HostServer mode if the mode is set to HostServer AND the server is running.
/// (checking if the mode is set to HostServer is not enough, it just means that the server plugin
/// and client plugin are running in the same App)
pub fn is_host_server(identity: Option<Res<NetworkIdentityState>>) -> bool {
    identity.is_some_and(|i| matches!(*i, NetworkIdentityState::HostServer))
}

/// Returns true if the peer is a client (host-server counts as a server)
pub(crate) fn is_client_ref(identity: Option<Ref<NetworkIdentityState>>) -> bool {
    todo!();
    // identity.is_some_and(|i| i.get() == &NetworkIdentityState::Client)
}

pub(crate) fn is_server_ref(identity: Option<Ref<NetworkIdentityState>>) -> bool {
    todo!();
    // identity.is_some_and(|i| i.get() != &NetworkIdentityState::Client)
}

pub(crate) fn is_host_server_ref(identity: Option<Ref<NetworkIdentityState>>) -> bool {
    todo!();
    // identity.is_some_and(|i| i.get() == &NetworkIdentityState::HostServer)
}

// TODO: how to get the state?
//  - if Server Started, we check if one of the connected clients is also a Client?
//  - on every Connection event, check if the connection is a Client

/// State that will contain the current role of the peer. This state is only active if the peer is connected
#[derive(Clone, PartialEq, Eq, Hash, Debug, Default, Resource)]
pub enum NetworkIdentityState {
    #[default]
    Other,
    /// The app only has a single Client
    Client,
    /// The app only has a single Server
    Server,
    /// The app is a host server
    HostServer,
}

#[derive(SystemParam)]
pub struct NetworkIdentity<'w> {
    identity: Option<Res<'w, NetworkIdentityState>>,
}

impl NetworkIdentity<'_> {
    pub fn is_client(&self) -> bool {
        todo!()
        // self.identity
        //     .as_ref()
        //     .is_some_and(|i| i.get() == &NetworkIdentityState::Client)
    }
    pub fn is_server(&self) -> bool {
        todo!()
        // self.identity
        //     .as_ref()
        //     .is_some_and(|i| i.get() != &NetworkIdentityState::Client)
    }
    pub fn is_host_server(&self) -> bool {
        todo!()
        // self.identity
        //     .as_ref()
        //     .is_some_and(|i| i.get() == &NetworkIdentityState::HostServer)
    }
}

pub trait AppIdentityExt {
    fn is_client(&self) -> bool;

    fn is_server(&self) -> bool;

    fn is_host_server(&self) -> bool;
}

impl AppIdentityExt for World {
    fn is_client(&self) -> bool {
        todo!()
        // self.get_resource::<NetworkIdentityState>()
        //     .is_some_and(|i| i.get() == &NetworkIdentityState::Client)
    }

    fn is_server(&self) -> bool {
        todo!()
        // self.get_resource::<NetworkIdentityState>()
        //     .is_some_and(|i| i.get() != &NetworkIdentityState::Client)
    }

    /// We are in Host-Server mode (for Prediction) if there is one client with the HostServer component
    /// (Which gets added to a connection that has both ClientOf and Client, where the Server is started
    /// and the Client is connected)
    fn is_host_server(&self) -> bool {
        todo!()
        // self.get_resource::<NetworkIdentityState>()
        //     .is_some_and(|i| i.get() == &NetworkIdentityState::HostServer)
    }
}
