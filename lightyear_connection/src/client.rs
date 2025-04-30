#[cfg(not(feature = "std"))]
use alloc::string::String;
use bevy::ecs::component::HookContext;
use bevy::ecs::world::DeferredWorld;
use bevy::prelude::{Component, Event, Reflect};
use lightyear_core::id::PeerId;
use lightyear_link::prelude::Unlinked;


/// Errors related to the client connection
#[derive(thiserror::Error, Debug)]
pub enum ConnectionError {
    #[error("io is not initialized")]
    IoNotInitialized,
    #[error("connection not found")]
    NotFound,
    #[error("client is not connected")]
    NotConnected,
}

#[derive(Default, Debug, Clone, Copy, PartialEq, Eq, Reflect)]
pub enum ClientState {
    /// Client is connected to the server
    Connected(PeerId),
    /// Client is connecting to the server
    Connecting,
    #[default]
    /// Client is disconnected from the server
    Disconnected,
}

/// Marker component to identify this entity as a Client
#[derive(Component, Default, Reflect)]
pub struct Client {
    pub state: ClientState
}

impl Client {
    pub fn peer_id(&self) -> Option<PeerId> {
        match self.state {
            ClientState::Connected(peer_id) => Some(peer_id),
            ClientState::Connecting => None,
            ClientState::Disconnected => None,
        }
    }
}


/// Trigger to connect the client
#[derive(Event)]
pub struct Connect;

/// Trigger to disconnect the client
#[derive(Event)]
pub struct Disconnect;


#[derive(Component, Event, Default, Debug, Reflect)]
#[component(on_add = Connected::on_add)]
pub struct Connected {
    pub peer_id: PeerId,
}

impl Connected {
    fn on_add(mut world: DeferredWorld, context: HookContext) {
        let peer_id = world.get::<Connected>(context.entity).unwrap().peer_id;
        if let Some(mut client) = world.get_mut::<Client>(context.entity) {
            client.state = ClientState::Connected(peer_id);
        };
        world.commands().entity(context.entity)
            .remove::<(Connecting, Disconnected)>();
    }
}

// TODO: add automatic disconnection for entities that are Connecting for too long
#[derive(Component, Event, Default, Debug, Reflect)]
#[component(on_add = Connecting::on_add)]
pub struct Connecting;

impl Connecting {
    fn on_add(mut world: DeferredWorld, context: HookContext) {
        if let Some(mut client) = world.get_mut::<Client>(context.entity) {
            client.state = ClientState::Connecting;
        }
        world.commands().entity(context.entity)
            .remove::<(Connected, Disconnected)>();
    }
}

#[derive(Component, Event, Default, Debug, Reflect)]
#[component(on_add = Disconnected::on_add)]
pub struct Disconnected {
    pub reason: Option<String>,
}

impl Disconnected {
    fn on_add(mut world: DeferredWorld, context: HookContext) {
        if let Some(mut client) = world.get_mut::<Client>(context.entity) {
            client.state = ClientState::Disconnected;
        }
        world.commands().entity(context.entity)
            .remove::<(Connecting, Connected)>();
    }
}


#[cfg(test)]
mod tests {
    #[test]
    fn test_connection() {

    }
}
