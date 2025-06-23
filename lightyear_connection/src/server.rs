use crate::client::{Client, Disconnected, Disconnecting};
use crate::client_of::ClientOf;
use bevy_app::{App, Last, Plugin};
use bevy_ecs::{
    component::{Component, HookContext},
    entity::Entity,
    event::Event,
    observer::Trigger,
    query::With,
    system::{Commands, Query},
    world::{DeferredWorld, OnAdd},
};
use bevy_reflect::Reflect;
use core::fmt::Debug;
use lightyear_link::prelude::Server;
use lightyear_link::{LinkStart, Unlinked};
use tracing::trace;

/// Errors related to the server connection
#[derive(thiserror::Error, Debug)]
pub enum ConnectionError {
    #[error("io is not initialized")]
    IoNotInitialized,
    #[error("connection not found")]
    ConnectionNotFound,
    #[error("the connection type for this client is invalid")]
    InvalidConnectionType,
}

/// Trigger to start the server
#[derive(Event)]
pub struct Start;

/// Trigger to stop the server
#[derive(Event)]
pub struct Stop;

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

#[derive(Component, Event, Reflect)]
#[component(on_add = Started::on_add)]
pub struct Started;

impl Started {
    fn on_add(mut world: DeferredWorld, context: HookContext) {
        trace!("Started added: removing Starting/Stopped");
        world
            .commands()
            .entity(context.entity)
            .remove::<(Starting, Stopped, Stopping)>();
    }
}

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

#[derive(Component, Event, Reflect)]
#[component(on_add = Stopped::on_add)]
pub struct Stopped;

impl Stopped {
    fn on_add(mut world: DeferredWorld, context: HookContext) {
        trace!("Stopped added: removing Started/Starting");
        world
            .commands()
            .entity(context.entity)
            .remove::<(Started, Starting, Stopping)>();
    }
}

pub struct ConnectionPlugin;

impl ConnectionPlugin {
    /// When the start request to Start, we also start the ServerLink.
    /// We also despawn any existing ClientOf.
    fn start(trigger: Trigger<Start>, mut commands: Commands) {
        trace!("Triggering LinkStart because Start was triggered");
        commands.trigger_targets(LinkStart, trigger.target());

        // TODO: this was a crutch to make sure that all ClientOfs are despawned when Stop is called..
        // commands.entity(trigger.target()).despawn_related::<Server>();
    }

    /// If the underlying link fails, we also stop the server
    fn stop_if_link_fails(
        trigger: Trigger<OnAdd, Unlinked>,
        // TODO: is Start/Stop reserved for the `Server` and not the `ServerLink`?
        query: Query<(), (With<Server>, With<Started>)>,
        mut commands: Commands,
    ) {
        if let Ok(()) = query.get(trigger.target()) {
            trace!("Triggering Stopped because Unlinked was triggered");
            commands.entity(trigger.target()).insert(Stopped);
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

/// RunCondition to check if the app is a server.
///
/// Note that the app could also have a host-client
pub fn is_server(server_query: Query<(), With<Server>>) -> bool {
    server_query.single().is_ok()
}

/// RunCondition to check if the app is a headless server:
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
