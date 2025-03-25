#[cfg(not(feature = "std"))]
use alloc::string::String;
#[cfg(not(feature = "std"))]
use alloc::{vec, vec::Vec};
use bevy::ecs::entity::EntitySetIterator;
use bevy::prelude::{Component, Entity, RelationshipTarget, Resource};
use core::fmt::Debug;
use serde::{Deserialize, Serialize};

use crate::id::ClientId;


#[derive(Component, Clone, PartialEq, Eq, Debug)]
#[cfg_attr(feature = "bevy_reflect", derive(bevy_reflect::Reflect))]
#[cfg_attr(
    feature = "bevy_reflect",
    reflect(Component, PartialEq, Debug, FromWorld, Clone)
)]
#[relationship(relationship_target = Clients)]
pub struct ClientOf {
    /// The server entity that this client is connected to
    #[relationship]
    pub server: Entity,
    /// The client id of the client
    pub id: ClientId,
}

#[derive(Component, Default, Debug, PartialEq, Eq)]
#[relationship_target(relationship = ClientOf, linked_spawn)]
#[cfg_attr(feature = "bevy_reflect", derive(bevy_reflect::Reflect))]
#[cfg_attr(feature = "bevy_reflect", reflect(Component, FromWorld, Default))]
pub struct Clients(Vec<Entity>);


#[derive(Component)]
struct ConnectedOn;

/// Reasons for denying a connection request
#[derive(Debug, PartialEq, Clone, Serialize, Deserialize)]
pub enum DeniedReason {
    ServerFull,
    Banned,
    InternalError,
    AlreadyConnected,
    TokenAlreadyUsed,
    InvalidToken,
    Custom(String),
}

/// Trait for handling connection requests from clients.
pub trait ConnectionRequestHandler: Debug + Send + Sync {
    /// Handle a connection request from a client.
    /// Returns None if the connection is accepted,
    /// Returns Some(reason) if the connection is denied.
    fn handle_request(&self, client_id: ClientId) -> Option<DeniedReason>;
}

/// By default, all connection requests are accepted by the server.
#[derive(Debug, Clone)]
pub struct DefaultConnectionRequestHandler;

impl ConnectionRequestHandler for DefaultConnectionRequestHandler {
    fn handle_request(&self, client_id: ClientId) -> Option<DeniedReason> {
        None
    }
}


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

#[cfg(test)]
mod tests {
    use crate::connection::server::{NetServer, ServerConnections};
    use crate::prelude::ClientId;
    use crate::tests::stepper::{BevyStepper, TEST_CLIENT_ID};
    use crate::transport::LOCAL_SOCKET;
    #[cfg(not(feature = "std"))]
    use alloc::vec;

    // Check that the server can successfully disconnect a client
    // and that there aren't any excessive logs afterwards
    // Enable logging to see if the logspam is fixed!
    #[test]
    fn test_server_disconnect_client() {
        // tracing_subscriber::FmtSubscriber::builder()
        //     .with_max_level(tracing::Level::INFO)
        //     .init();
        let mut stepper = BevyStepper::default();
        stepper
            .server_app
            .world_mut()
            .resource_mut::<ServerConnections>()
            .disconnect(ClientId::Netcode(TEST_CLIENT_ID))
            .unwrap();
        // make sure the server disconnected the client
        for _ in 0..10 {
            stepper.frame_step();
        }
        assert_eq!(
            stepper
                .server_app
                .world_mut()
                .resource_mut::<ServerConnections>()
                .servers[0]
                .connected_client_ids(),
            vec![]
        );
    }

    #[test]
    fn test_server_get_client_addr() {
        let mut stepper = BevyStepper::default();
        assert_eq!(
            stepper
                .server_app
                .world_mut()
                .resource_mut::<ServerConnections>()
                .client_addr(ClientId::Netcode(TEST_CLIENT_ID))
                .unwrap(),
            LOCAL_SOCKET
        );
    }
}
