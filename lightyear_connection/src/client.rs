use bevy::ecs::component::HookContext;
use bevy::ecs::world::DeferredWorld;
use bevy::prelude::{Component, Event};
use lightyear_core::id::PeerId;


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

#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
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
#[derive(Component, Default)]
pub struct Client {
    pub state: ClientState
}


/// Trigger to connect the client
#[derive(Event)]
pub struct Connect;

/// Trigger to disconnect the client
#[derive(Event)]
pub struct Disconnect;

// TODO: on_add: remove Connecting/Disconnected
#[derive(Component, Event, Default, Debug)]
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
    }
}

// TODO: add automatic disconnection for entities that are Connecting for too long
#[derive(Component, Event, Default, Debug)]
#[component(on_add = Connecting::on_add)]
pub struct Connecting;

impl Connecting {
    fn on_add(mut world: DeferredWorld, context: HookContext) {
        if let Some(mut client) = world.get_mut::<Client>(context.entity) {
            client.state = ClientState::Connecting;
        }
    }
}

#[derive(Component, Event, Default, Debug)]
#[component(on_add = Disconnected::on_add)]
pub struct Disconnected;

impl Disconnected {
    fn on_add(mut world: DeferredWorld, context: HookContext) {
        if let Some(mut client) = world.get_mut::<Client>(context.entity) {
            client.state = ClientState::Disconnected;
        }
    }
}


#[cfg(test)]
mod tests {
    #[test]
    fn test_connection() {

    }
}
