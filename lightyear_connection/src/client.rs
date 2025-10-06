use alloc::{format, string::String};
use bevy_app::{App, Plugin};
use bevy_ecs::lifecycle::HookContext;
use bevy_ecs::prelude::*;
use bevy_ecs::{reflect::ReflectResource, world::DeferredWorld};
use bevy_platform::collections::HashMap;
use bevy_reflect::Reflect;
use lightyear_core::id::{PeerId, RemoteId};
use lightyear_link::LinkStart;
use lightyear_link::prelude::{Server, Unlinked};
use tracing::trace;

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
    Connected,
    /// Client is connecting to the server
    Connecting,
    Disconnecting,
    #[default]
    /// Client is disconnected from the server
    Disconnected,
}

/// Marker component to identify this entity as a Client
#[derive(Component, Default, Reflect)]
pub struct Client {
    pub state: ClientState,
}

/// Trigger to connect the client
#[derive(EntityEvent)]
pub struct Connect {
    pub entity: Entity,
}

/// Trigger to disconnect the client
#[derive(EntityEvent)]
pub struct Disconnect {
    pub entity: Entity,
}

// TODO: it looks like in some cases, we want Connected.peer_id to return the local peer_id (when client connects to server)
//  and in some cases we want it to return the remote peer_id (when server's ClientOf gets connected)
//  We should decide on a rule.

#[derive(Component, Debug, Reflect)]
#[component(on_add = Connected::on_add)]
pub struct Connected;

impl Connected {
    fn on_add(mut world: DeferredWorld, context: HookContext) {
        let peer_id = world
            .get::<RemoteId>(context.entity)
            .unwrap_or_else(|| {
                panic!(
                    "A Connected entity ({:?}) must always have a RemoteId component",
                    context.entity
                )
            })
            .0;
        if let Some(mut client) = world.get_mut::<Client>(context.entity) {
            client.state = ClientState::Connected;
        };
        world
            .commands()
            .entity(context.entity)
            .remove::<(Connecting, Disconnected)>();
        if let Some(mut metadata) = world.get_resource_mut::<PeerMetadata>() {
            metadata.mapping.insert(peer_id, context.entity);
        }
    }
}

// TODO: add automatic disconnection for entities that are Connecting for too long
#[derive(Component, Default, Debug, Reflect)]
#[component(on_add = Connecting::on_add)]
pub struct Connecting;

impl Connecting {
    fn on_add(mut world: DeferredWorld, context: HookContext) {
        if let Some(mut client) = world.get_mut::<Client>(context.entity) {
            client.state = ClientState::Connecting;
        }
        world
            .commands()
            .entity(context.entity)
            .remove::<(Connected, Disconnecting, Disconnected)>();
    }
}

#[derive(Component, Default, Debug, Reflect)]
#[component(on_add = Disconnected::on_add)]
pub struct Disconnected {
    pub reason: Option<String>,
}

impl Disconnected {
    fn on_add(mut world: DeferredWorld, context: HookContext) {
        if let Some(mut client) = world.get_mut::<Client>(context.entity) {
            client.state = ClientState::Disconnected;
        }
        if let Some(peer_id) = world.get::<RemoteId>(context.entity).map(|c| c.0) {
            world
                .resource_mut::<PeerMetadata>()
                .mapping
                .remove(&peer_id);
        }
        world
            .commands()
            .entity(context.entity)
            .remove::<(Connecting, Disconnecting, Connected)>();
    }
}

#[derive(Component, Default, Debug, Reflect)]
#[component(on_add = Disconnecting::on_add)]
pub struct Disconnecting;

impl Disconnecting {
    fn on_add(mut world: DeferredWorld, context: HookContext) {
        if let Some(mut client) = world.get_mut::<Client>(context.entity) {
            client.state = ClientState::Disconnecting;
        }
        world
            .commands()
            .entity(context.entity)
            .remove::<(Connected, Connecting, Disconnected)>();
    }
}

/// Resource that maintains a mapping from a remote PeerId to the corresponding local Entity
/// that is connected to that peer
#[derive(Resource, Debug, Default, Reflect)]
#[reflect(Resource)]
pub struct PeerMetadata {
    pub mapping: HashMap<PeerId, Entity>,
}

pub struct ConnectionPlugin;

impl ConnectionPlugin {
    /// When the client request to connect, we also try to establish the link
    fn connect(connect: On<Connect>, mut commands: Commands) {
        trace!("Triggering LinkStart because Connect was triggered");
        commands.trigger(LinkStart {
            entity: connect.entity,
        });
    }

    /// If the underlying link fails, we also disconnect the client
    fn disconnect_if_link_fails(
        trigger: On<Add, Unlinked>,
        query: Query<&Unlinked, (Without<Disconnected>, Without<Server>)>,
        mut commands: Commands,
    ) {
        if let Ok(unlinked) = query.get(trigger.entity) {
            trace!(
                "Adding Disconnected because the link got Unlinked (reason: {:?})",
                unlinked.reason
            );
            commands.entity(trigger.entity).insert(Disconnected {
                reason: Some(format!("Link failed: {:?}", unlinked.reason)),
            });
        }
    }
}

impl Plugin for ConnectionPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PeerMetadata>();
        app.add_observer(Self::connect);
        app.add_observer(Self::disconnect_if_link_fails);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_connection() {}
}
