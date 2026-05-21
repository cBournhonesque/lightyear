//! Client-side connection lifecycle state.
//!
//! This module defines the common lifecycle components used by concrete client connection crates.
//! Protocol implementations such as `lightyear_raw_connection`, `lightyear_netcode`, and
//! `lightyear_steam` react to [`crate::client::Connect`] and [`crate::client::Disconnect`]
//! triggers, then insert [`crate::client::Connecting`], [`crate::client::Connected`],
//! [`crate::client::Disconnecting`], or [`crate::client::Disconnected`] as the underlying protocol
//! progresses.
//!
//! The marker components keep [`crate::client::Client`] synchronized through component hooks, so
//! systems can either query marker components directly or inspect the aggregate state on the client
//! component.

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
#[allow(unused_imports)]
use tracing::{info, trace};

/// Errors related to client connection lifecycle operations.
#[derive(thiserror::Error, Debug)]
pub enum ConnectionError {
    /// The concrete IO or link backend has not been initialized yet.
    #[error("io is not initialized")]
    IoNotInitialized,
    /// The requested connection entity or peer could not be found.
    #[error("connection not found")]
    NotFound,
    /// The client is not connected to a server.
    #[error("client is not connected")]
    NotConnected,
}

/// Aggregate client connection state mirrored from lifecycle marker components.
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq, Reflect)]
pub enum ClientState {
    /// The client is connected to the server.
    Connected,
    /// The client is in a protocol-specific handshake or link-start phase.
    Connecting,
    /// The client is disconnecting and may still need to flush protocol packets.
    Disconnecting,
    #[default]
    /// The client is disconnected from the server.
    Disconnected,
}

/// Component identifying an entity as a client connection.
///
/// Concrete connection crates usually add this through component requirements on their own client IO
/// component. The [`state`](Self::state) field is updated by lifecycle marker hooks.
#[derive(Component, Default, Reflect)]
pub struct Client {
    /// Aggregate state matching the active lifecycle marker component.
    pub state: ClientState,
}

/// Entity trigger requesting that a client starts connecting.
///
/// The shared client plugin translates this into [`LinkStart`]. Concrete
/// protocols may also observe the trigger to start their own handshake state.
#[derive(EntityEvent)]
pub struct Connect {
    /// Client entity to connect.
    pub entity: Entity,
}

/// Entity trigger requesting that a client disconnects.
#[derive(EntityEvent)]
pub struct Disconnect {
    /// Client entity to disconnect.
    pub entity: Entity,
}

// TODO: it looks like in some cases, we want Connected.peer_id to return the local peer_id (when client connects to server)
//  and in some cases we want it to return the remote peer_id (when server's ClientOf gets connected)
//  We should decide on a rule.

/// Marker component for a client whose connection is established.
///
/// Adding this marker updates [`Client::state`], removes incompatible lifecycle markers, and records
/// the entity in [`PeerMetadata`] using its [`RemoteId`].
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

/// Marker component for a client that is currently connecting.
///
/// Adding this marker updates [`Client::state`] and removes incompatible lifecycle markers.
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

/// Marker component for a client that is disconnected.
///
/// Adding this marker updates [`Client::state`], removes the peer mapping from [`PeerMetadata`], and
/// clears incompatible lifecycle markers.
#[derive(Component, Default, Debug, Reflect)]
#[component(on_add = Disconnected::on_add)]
pub struct Disconnected {
    /// Optional diagnostic reason for the disconnection.
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

/// Marker component for a client that is in the process of disconnecting.
///
/// Server-side client entities can use this as a one-frame state so disconnect packets or observers
/// can run before the entity is despawned.
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

/// Mapping from remote peers to local connection entities.
///
/// The map is maintained by [`Connected`] and [`Disconnected`] hooks. It is used by targeting and
/// routing code that starts from a [`PeerId`] and needs to find the corresponding Bevy entity.
#[derive(Resource, Debug, Default, Reflect)]
#[reflect(Resource)]
pub struct PeerMetadata {
    /// Remote peer ID to local connection entity.
    pub mapping: HashMap<PeerId, Entity>,
}

/// Client lifecycle plugin.
///
/// The plugin installs observers that translate [`Connect`] into
/// [`LinkStart`] and convert link failures into [`Disconnected`].
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
                entity = ?trigger.entity,
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
