#[cfg(not(feature = "std"))]
use alloc::string::String;
use bevy::app::{App, Plugin};
use bevy::ecs::component::HookContext;
use bevy::ecs::world::DeferredWorld;
use bevy::prelude::{Commands, Component, Event, OnAdd, Query, Reflect, Trigger};
use lightyear_core::id::PeerId;
use lightyear_link::prelude::Unlinked;
use lightyear_link::LinkStart;


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


pub struct ConnectionPlugin;

impl ConnectionPlugin {
    /// When the client request to connect, we also try to establis the link
    fn connect(trigger: Trigger<Connect>, mut commands: Commands) {
        commands.trigger_targets(LinkStart, trigger.target());
    }
    
    /// If the underlying link fails, we also disconnect the client
    fn disconnect_if_link_fails(
        trigger: Trigger<OnAdd, Unlinked>,
        query: Query<&Unlinked>,
        mut commands: Commands
    ) {
        if let Ok(unlinked) = query.get(trigger.target()) {
            commands.entity(trigger.target())
                .insert(Disconnected {
                    reason: Some(format!("Link failed: {:?}", unlinked.reason))
                });
        }
    }
}

impl Plugin for ConnectionPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(Self::connect);
        app.add_observer(Self::disconnect_if_link_fails);
    }
}


#[cfg(test)]
mod tests {
    #[test]
    fn test_connection() {

    }
}
