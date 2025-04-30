use crate::client::{Client, ClientState, Connect, Connected, Connecting, Disconnected};
use crate::direction::NetworkDirection;
#[cfg(not(feature = "std"))]
use alloc::string::String;
#[cfg(not(feature = "std"))]
use alloc::{vec, vec::Vec};
use bevy::app::{App, Plugin};
use bevy::ecs::component::HookContext;
use bevy::ecs::world::DeferredWorld;
use bevy::prelude::{Commands, Component, Event, OnAdd, Query, Reflect, Trigger};
use core::fmt::Debug;
use lightyear_link::prelude::ServerLink;
use lightyear_link::{LinkStart, Unlinked};
use tracing::info;

/// A dummy connection plugin that takes payloads directly from the Link
/// to the Transport without any processing
pub struct PassthroughClientPlugin;


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
        world.commands().entity(context.entity)
            .remove::<(Started, Stopped)>();
    }
}

#[derive(Component, Event, Reflect)]
#[component(on_add = Started::on_add)]
pub struct Started;

impl Started {
    fn on_add(mut world: DeferredWorld, context: HookContext) {
        world.commands().entity(context.entity)
            .remove::<(Starting, Stopped)>();
    }
}

#[derive(Component, Event, Reflect)]
#[component(on_add = Stopped::on_add)]
pub struct Stopped;

impl Stopped {
    fn on_add(mut world: DeferredWorld, context: HookContext) {
        world.commands().entity(context.entity)
            .remove::<(Started, Starting)>();
    }
}


pub struct ConnectionPlugin;

impl ConnectionPlugin {
    /// When the start request to start, we also start the ServerLink
    fn start(trigger: Trigger<Start>, mut commands: Commands) {
        info!("Triggering LinkStart");
        commands.trigger_targets(LinkStart, trigger.target());
    }

    /// If the underlying link fails, we also stop the server
    fn stop_if_link_fails(
        trigger: Trigger<OnAdd, Unlinked>,
        mut commands: Commands
    ) {
        commands.entity(trigger.target()).insert(Stopped);
    }
}

impl Plugin for ConnectionPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(Self::start);
        app.add_observer(Self::stop_if_link_fails);
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
