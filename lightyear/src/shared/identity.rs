use crate::prelude::{client, server};
use bevy::ecs::system::SystemParam;
use bevy::prelude::{ComputedStates, Res, State, World};


/// State that will contain the current role of the peer. This state is only active if the peer is connected
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum NetworkIdentityState {
    Client,
    Server,
    HostServer,
}

impl ComputedStates for NetworkIdentityState {
    type SourceStates = (Option<client::NetworkingState>, Option<server::NetworkingState>);

    /// We then define the compute function, which takes in the AppState
    fn compute(sources: Self::SourceStates) -> Option<Self> {
        match sources {
            // If client and server states are both present and started, then we must be a HostServer
            (Some(client::NetworkingState::Connected), Some(server::NetworkingState::Started)) => Some(NetworkIdentityState::HostServer),
            // we include these so that we can run the host_server disconnection systems
            (Some(client::NetworkingState::Connected), Some(server::NetworkingState::Stopping)) => Some(NetworkIdentityState::HostServer),
            (Some(client::NetworkingState::Disconnecting), Some(server::NetworkingState::Started)) => Some(NetworkIdentityState::HostServer),
            // If only the client is connected, we are a Client
            (Some(client::NetworkingState::Connected), _) => Some(NetworkIdentityState::Client),
            // If only the server is started, we are a Server
            (_, Some(server::NetworkingState::Started)) => Some(NetworkIdentityState::Server),
            // If neither client or server are connected, then we don't want the `NetworkIdentity` state to exist
            _ => None
        }
    }
}


#[derive(SystemParam)]
pub struct NetworkIdentity<'w> {
    identity: Option<Res<'w, State<NetworkIdentityState>>>
}

impl NetworkIdentity<'_> {
    pub fn is_client(&self) -> bool {
        self.identity.as_ref().is_some_and(|i| i.get() == &NetworkIdentityState::Client)
    }
    pub fn is_server(&self) -> bool {
        self.identity.as_ref().is_some_and(|i| i.get() != &NetworkIdentityState::Client)
    }
    pub fn is_host_server(&self) -> bool {
        self.identity.as_ref().is_some_and(|i| i.get() == &NetworkIdentityState::HostServer)
    }
}

pub trait AppIdentityExt {
    fn is_client(&self) -> bool;

    fn is_server(&self) -> bool;

    fn is_host_server(&self) -> bool;

}

impl AppIdentityExt for World {
    fn is_client(&self) -> bool {
        self.get_resource::<State<NetworkIdentityState>>().is_some_and(|i| i.get() == &NetworkIdentityState::Client)
    }

    fn is_server(&self) -> bool {
        self.get_resource::<State<NetworkIdentityState>>().is_some_and(|i| i.get() != &NetworkIdentityState::Client)
    }

    fn is_host_server(&self) -> bool {
        self.get_resource::<State<NetworkIdentityState>>().is_some_and(|i| i.get() == &NetworkIdentityState::HostServer)
    }
}
