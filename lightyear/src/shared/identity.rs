use crate::prelude::{client, server};
use bevy::prelude::{ComputedStates, State, World};


/// State that will contain the current role of the peer. This state is only active if the peer is connected
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum NetworkIdentity {
    Client,
    Server,
    HostServer,
}

impl ComputedStates for NetworkIdentity {
    type SourceStates = (Option<client::NetworkingState>, Option<server::NetworkingState>);

    /// We then define the compute function, which takes in the AppState
    fn compute(sources: Self::SourceStates) -> Option<Self> {
        match sources {
            // If client and server states are both present and started, then we must be a HostServer
            (Some(client::NetworkingState::Connected), Some(server::NetworkingState::Started)) => Some(NetworkIdentity::HostServer),
            // we include these so that we can run the host_server disconnection systems
            (Some(client::NetworkingState::Connected), Some(server::NetworkingState::Stopping)) => Some(NetworkIdentity::HostServer),
            (Some(client::NetworkingState::Disconnecting), Some(server::NetworkingState::Started)) => Some(NetworkIdentity::HostServer),
            // If only the client is connected, we are a Client
            (Some(client::NetworkingState::Connected), _) => Some(NetworkIdentity::Client),
            // If only the server is started, we are a Server
            (_, Some(server::NetworkingState::Started)) => Some(NetworkIdentity::Server),
            // If neither client or server are connected, then we don't want the `NetworkIdentity` state to exist
            _ => None
        }
    }
}


pub trait AppIdentityExt {
    fn is_client(&self) -> bool;

    fn is_server(&self) -> bool;

    fn is_host_server(&self) -> bool;

}

impl AppIdentityExt for World {
    fn is_client(&self) -> bool {
        self.get_resource::<State<NetworkIdentity>>().is_some_and(|i| i.get() == &NetworkIdentity::Client)
    }

    fn is_server(&self) -> bool {
        self.get_resource::<State<NetworkIdentity>>().is_some_and(|i| i.get() != &NetworkIdentity::Client)
    }

    fn is_host_server(&self) -> bool {
        self.get_resource::<State<NetworkIdentity>>().is_some_and(|i| i.get() == &NetworkIdentity::HostServer)
    }
}
