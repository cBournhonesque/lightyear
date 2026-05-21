//! Server-side connection lifecycle state.
//!
//! This module defines the common lifecycle components used by concrete server connection crates.
//! Protocol implementations such as `lightyear_raw_connection`, `lightyear_netcode`, and
//! `lightyear_steam` react to [`crate::server::Start`] and [`crate::server::Stop`] triggers, then
//! insert [`crate::server::Starting`], [`crate::server::Started`],
//! [`crate::server::Stopping`], or [`crate::server::Stopped`] as the underlying link/protocol
//! changes state.
//!
//! Server-side client links are represented by child entities marked with
//! [`ClientOf`](crate::client_of::ClientOf) and the client lifecycle markers from
//! [`crate::client`].

use crate::client::{Client, Disconnected, Disconnecting, PeerMetadata};
use crate::client_of::ClientOf;
use bevy_app::{App, Last, Plugin};
use bevy_ecs::lifecycle::HookContext;
use bevy_ecs::prelude::*;
use bevy_ecs::world::DeferredWorld;
use bevy_reflect::Reflect;
use core::fmt::Debug;
use lightyear_core::id::PeerId;
use lightyear_link::prelude::Server;
use lightyear_link::{LinkStart, Unlinked};
use tracing::trace;

/// Errors related to server connection lifecycle operations.
#[derive(thiserror::Error, Debug)]
pub enum ConnectionError {
    /// The concrete IO or link backend has not been initialized yet.
    #[error("io is not initialized")]
    IoNotInitialized,
    /// The requested client or server connection could not be found.
    #[error("connection not found")]
    ConnectionNotFound,
    /// A client entity is not using the connection type expected by this server protocol.
    #[error("the connection type for this client is invalid")]
    InvalidConnectionType,
}

/// Entity trigger requesting that a server starts listening or accepting connections.
#[derive(EntityEvent)]
pub struct Start {
    /// Server entity to start.
    pub entity: Entity,
}

/// Entity trigger requesting that a server stops and disconnects clients.
#[derive(EntityEvent)]
pub struct Stop {
    /// Server entity to stop.
    pub entity: Entity,
}

/// Marker component for a server that is starting.
///
/// Adding this marker removes incompatible server lifecycle markers.
#[derive(Component)]
#[component(on_add = Starting::on_add)]
pub struct Starting;

impl Starting {
    fn on_add(mut world: DeferredWorld, context: HookContext) {
        trace!("Starting added: removing Started/Stopped");
        world
            .commands()
            .entity(context.entity)
            .remove::<(Started, Stopped, Stopping)>();
    }
}

/// Marker component for a server whose connection layer is active.
///
/// Adding this marker inserts `PeerId::Server` into [`PeerMetadata`] and removes incompatible
/// server lifecycle markers.
#[derive(Component, Event, Reflect)]
#[component(on_add = Started::on_add)]
pub struct Started;

impl Started {
    fn on_add(mut world: DeferredWorld, context: HookContext) {
        world
            .resource_mut::<PeerMetadata>()
            .mapping
            .insert(PeerId::Server, context.entity);
        trace!("Started added: removing Starting/Stopped");
        world
            .commands()
            .entity(context.entity)
            .remove::<(Starting, Stopped, Stopping)>();
    }
}

/// Marker component for a server that is stopping.
///
/// Adding this marker removes incompatible server lifecycle markers while concrete protocols flush
/// disconnect packets or close their underlying IO.
#[derive(Component, Event, Reflect)]
#[component(on_add = Stopping::on_add)]
pub struct Stopping;

impl Stopping {
    fn on_add(mut world: DeferredWorld, context: HookContext) {
        trace!("Stopping added: removing Started/Starting");
        world
            .commands()
            .entity(context.entity)
            .remove::<(Started, Starting, Stopped)>();
    }
}

/// Marker component for a server whose connection layer is stopped.
///
/// Adding this marker removes `PeerId::Server` from [`PeerMetadata`] and clears incompatible server
/// lifecycle markers.
#[derive(Component, Event, Reflect)]
#[component(on_add = Stopped::on_add)]
pub struct Stopped;

impl Stopped {
    fn on_add(mut world: DeferredWorld, context: HookContext) {
        world
            .resource_mut::<PeerMetadata>()
            .mapping
            .remove(&PeerId::Server);
        trace!("Stopped added: removing Started/Starting");
        world
            .commands()
            .entity(context.entity)
            .remove::<(Started, Starting, Stopping)>();
    }
}

/// Server lifecycle plugin.
///
/// The plugin installs observers that translate [`Start`] into
/// [`LinkStart`], convert link failures into [`Stopped`], and despawn
/// server-side client entities after they spend one frame in [`Disconnecting`].
pub struct ConnectionPlugin;

impl ConnectionPlugin {
    /// When the start request to Start, we also start the ServerLink.
    /// We also despawn any existing ClientOf.
    fn start(trigger: On<Start>, mut commands: Commands) {
        trace!("Triggering LinkStart because Start was triggered");
        commands.trigger(LinkStart {
            entity: trigger.entity,
        });

        // TODO: this was a crutch to make sure that all ClientOfs are despawned when Stop is called..
        // commands.entity(trigger.entity).despawn_related::<Server>();
    }

    /// If the underlying link fails, we also stop the server
    fn stop_if_link_fails(
        trigger: On<Add, Unlinked>,
        // TODO: is Start/Stop reserved for the `Server` and not the `ServerLink`?
        query: Query<(), (With<Server>, With<Started>)>,
        mut commands: Commands,
    ) {
        if let Ok(()) = query.get(trigger.entity) {
            trace!("Triggering Stopped because Unlinked was triggered");
            commands.entity(trigger.entity).insert(Stopped);
        }
    }

    /// Despawn disconnecting clients after 1 frame of Disconnecting
    /// (We wait for 1 frame to make sure that any disconnection packets can be sent)
    fn disconnect(
        query: Query<Entity, (With<Disconnecting>, With<ClientOf>)>,
        mut commands: Commands,
    ) {
        for entity in query.iter() {
            trace!(
                "Set ClientOf entity {:?} to Disconnected and despawn",
                entity
            );
            // Set to Disconnected before despawning to trigger observers
            commands
                .entity(entity)
                .insert(Disconnected { reason: None })
                .despawn();
        }
    }
}

/// Run condition that returns `true` when the app has exactly one server entity.
///
/// Note that the app could also have a host-client
pub fn is_server(server_query: Query<(), With<Server>>) -> bool {
    server_query.single().is_ok()
}

/// Run condition that returns `true` when the app is a headless server:
/// - there is an entity with the `Server` component
/// - there are no entities with the `Client` component
pub fn is_headless_server(
    server_query: Query<(), With<Server>>,
    query: Query<(), With<Client>>,
) -> bool {
    server_query.single().is_ok() && query.is_empty()
}

impl Plugin for ConnectionPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(Self::start);
        app.add_observer(Self::stop_if_link_fails);
        app.add_systems(Last, Self::disconnect);
    }
}

// #[cfg(test)]
// mod tests {
//     use crate::connection::server::{NetServer, ServerConnections};
//     use crate::prelude::ClientId;
//     use crate::tests::stepper::{BevyStepper, TEST_CLIENT_ID};
//     use crate::transport::LOCAL_SOCKET;
//     #[cfg(not(feature = "std"))]
//     use alloc::vec;
//
//     // Check that the server can successfully disconnect a client
//     // and that there aren't any excessive logs afterwards
//     // Enable logging to see if the logspam is fixed!
//     #[test]
//     fn test_server_disconnect_client() {
//         // tracing_subscriber::FmtSubscriber::builder()
//         //     .with_max_level(tracing::Level::INFO)
//         //     .init();
//         let mut stepper = BevyStepper::default();
//         stepper
//             .server_app
//             .world_mut()
//             .resource_mut::<ServerConnections>()
//             .disconnect(ClientId::Netcode(TEST_CLIENT_ID))
//             .unwrap();
//         // make sure the server disconnected the client
//         for _ in 0..10 {
//             stepper.frame_step();
//         }
//         assert_eq!(
//             stepper
//                 .server_app
//                 .world_mut()
//                 .resource_mut::<ServerConnections>()
//                 .servers[0]
//                 .connected_client_ids(),
//             vec![]
//         );
//     }
//
//     #[test]
//     fn test_server_get_client_addr() {
//         let mut stepper = BevyStepper::default();
//         assert_eq!(
//             stepper
//                 .server_app
//                 .world_mut()
//                 .resource_mut::<ServerConnections>()
//                 .client_addr(ClientId::Netcode(TEST_CLIENT_ID))
//                 .unwrap(),
//             LOCAL_SOCKET
//         );
//     }
// }
