use crate::client::{Disconnected, DisconnectedReason, Disconnecting};
use crate::client_of::ClientOf;
use crate::network_topology::NetworkingMetadata;
use bevy_app::{App, Last, Plugin};
use bevy_ecs::lifecycle::HookContext;
use bevy_ecs::prelude::*;
use bevy_ecs::world::DeferredWorld;
use bevy_platform::collections::HashMap;
use bevy_reflect::Reflect;
use core::fmt::Debug;
use lightyear_core::id::PeerId;
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
#[derive(EntityEvent)]
pub struct Start {
    pub entity: Entity,
}

/// Trigger to stop the server
#[derive(EntityEvent)]
pub struct Stop {
    pub entity: Entity,
}

#[derive(Component)]
#[component(
    on_add = Starting::on_add,
    on_despawn = clear_server_mapping_on_despawn
)]
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
#[component(
    on_add = Started::on_add,
    on_despawn = clear_server_mapping_on_despawn
)]
pub struct Started;

impl Started {
    fn on_add(mut world: DeferredWorld, context: HookContext) {
        world
            .resource_mut::<NetworkingMetadata>()
            .peer_map
            .insert(PeerId::Server, context.entity);
        trace!("Started added: removing Starting/Stopped");
        world
            .commands()
            .entity(context.entity)
            .remove::<(Starting, Stopped, Stopping)>();
    }
}

#[derive(Component, Event, Reflect)]
#[component(
    on_add = Stopping::on_add,
    on_despawn = clear_server_mapping_on_despawn
)]
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
#[component(
    on_add = Stopped::on_add,
    on_despawn = clear_server_mapping_on_despawn
)]
pub struct Stopped;

impl Stopped {
    fn on_add(mut world: DeferredWorld, context: HookContext) {
        clear_server_mapping(
            &mut world
                .resource_mut::<NetworkingMetadata>()
                .peer_map,
            context.entity,
        );
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
                .insert(Disconnected {
                    reason: DisconnectedReason::UserRequested(None),
                })
                .despawn();
        }
    }
}

fn clear_server_mapping(peer_map: &mut HashMap<PeerId, Entity>, entity: Entity) {
    if peer_map.get(&PeerId::Server) == Some(&entity) {
        peer_map.remove(&PeerId::Server);
    }
}

fn clear_server_mapping_on_despawn(mut world: DeferredWorld, context: HookContext) {
    clear_server_mapping(
        &mut world
            .resource_mut::<NetworkingMetadata>()
            .peer_map,
        context.entity,
    );
}

#[deprecated(note = "Use `crate::identity::is_server` instead")]
pub use crate::identity::is_server;

#[deprecated(note = "Use `crate::identity::is_headless_server` instead")]
pub use crate::identity::is_headless_server;

impl Plugin for ConnectionPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(Self::start);
        app.add_observer(Self::stop_if_link_fails);
        app.add_systems(Last, Self::disconnect);
    }
}

#[cfg(test)]
mod tests {
    use super::{Started, Stopped, Stopping};
    use crate::network_topology::NetworkingMetadata;
    use bevy_app::App;
    use lightyear_core::id::PeerId;
    use lightyear_link::server::Server;

    #[test]
    fn server_mapping_is_cleared_when_stopped() {
        let mut app = App::new();
        app.init_resource::<NetworkingMetadata>();
        let server = app.world_mut().spawn((Server::default(), Started)).id();

        assert_eq!(
            app.world()
                .resource::<NetworkingMetadata>()
                .peer_map
                .get(&PeerId::Server),
            Some(&server)
        );

        app.world_mut().entity_mut(server).insert(Stopping);

        assert_eq!(
            app.world()
                .resource::<NetworkingMetadata>()
                .peer_map
                .get(&PeerId::Server),
            Some(&server)
        );

        app.world_mut().entity_mut(server).insert(Stopped);

        assert!(
            !app.world()
                .resource::<NetworkingMetadata>()
                .peer_map
                .contains_key(&PeerId::Server)
        );
    }

    #[test]
    fn despawned_server_clears_peer_map() {
        let mut app = App::new();
        app.init_resource::<NetworkingMetadata>();
        let server = app.world_mut().spawn((Server::default(), Started)).id();

        assert_eq!(
            app.world()
                .resource::<NetworkingMetadata>()
                .peer_map
                .get(&PeerId::Server),
            Some(&server)
        );

        app.world_mut().despawn(server);

        assert!(
            !app.world()
                .resource::<NetworkingMetadata>()
                .peer_map
                .contains_key(&PeerId::Server)
        );
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
