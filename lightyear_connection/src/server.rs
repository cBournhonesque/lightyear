use crate::client_of::ClientOf;
use crate::direction::NetworkDirection;
use crate::id::PeerId;
#[cfg(not(feature = "std"))]
use alloc::string::String;
#[cfg(not(feature = "std"))]
use alloc::{vec, vec::Vec};
use bevy::prelude::{Component, Event, OnAdd, Query, Res, Trigger};
use core::fmt::Debug;
use lightyear_messages::receive::MessageReceiver;
use lightyear_messages::registry::MessageRegistration;
use lightyear_messages::send::MessageSender;
use lightyear_messages::Message;
use lightyear_transport::channel::registry::ChannelRegistration;
use lightyear_transport::channel::Channel;
use lightyear_transport::prelude::{ChannelRegistry, Transport};
use serde::{Deserialize, Serialize};

impl ClientOf {
    pub(crate) fn add_sender_channel<C: Channel>(trigger: Trigger<OnAdd, ClientOf>, mut query: Query<&mut Transport>, registry: Res<ChannelRegistry>) {
        if let Ok(mut transport) = query.get_mut(trigger.target()) {
            transport.add_sender_from_registry::<C>(&registry)
        }
    }

    pub(crate) fn add_receiver_channel<C: Channel>(trigger: Trigger<OnAdd, ClientOf>, mut query: Query<&mut Transport>, registry: Res<ChannelRegistry>) {
        if let Ok(mut transport) = query.get_mut(trigger.target()) {
            transport.add_receiver_from_registry::<C>(&registry)
        }
    }
}


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
    fn handle_request(&self, client_id: PeerId) -> Option<DeniedReason>;
}

/// By default, all connection requests are accepted by the server.
#[derive(Debug, Clone)]
pub struct DefaultConnectionRequestHandler;

impl ConnectionRequestHandler for DefaultConnectionRequestHandler {
    fn handle_request(&self, client_id: PeerId) -> Option<DeniedReason> {
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

/// Trigger to start the server
#[derive(Event)]
pub struct Start;

/// Trigger to stop the server
#[derive(Event)]
pub struct Stop;

#[derive(Component)]
pub struct Starting;

#[derive(Component, Event)]
pub struct Started;

#[derive(Component, Event)]
pub struct Stopped;

#[derive(Component, Event)]
pub struct ClientConnected(pub PeerId);

#[derive(Component, Event)]
pub struct ClientDisconnected;

pub(crate) trait AppMessageDirectionExt {
    /// Add a new [`NetworkDirection`] to the registry
    fn add_direction(&mut self, direction: NetworkDirection);
}

impl<M: Message> AppMessageDirectionExt for MessageRegistration<'_, M> {
    // TODO: as much as possible, don't include server code for dedicated clients and vice-versa
    //   see how we can achieve this. Maybe half of the funciton is in lightyear_client and the other half in lightyear_server ?
    fn add_direction(&mut self, direction: NetworkDirection) {
        match direction {
            NetworkDirection::ClientToServer => {
                self.app.register_required_components::<ClientOf, MessageSender<M>>();
            }
            NetworkDirection::ServerToClient => {
                self.app.register_required_components::<ClientOf, MessageReceiver<M>>();
            }
            NetworkDirection::Bidirectional => {
                self.add_direction(NetworkDirection::ClientToServer);
                self.add_direction(NetworkDirection::ServerToClient);
            }
        }
    }
}

pub(crate) trait AppChannelDirectionExt {
    fn add_direction(&mut self, direction: NetworkDirection);
}

impl<C: Channel> AppChannelDirectionExt for ChannelRegistration<'_, C> {
    /// Add a new [`NetworkDirection`] to the registry
    fn add_direction(&mut self, direction: NetworkDirection) {
         match direction {
            NetworkDirection::ClientToServer => {
                self.app.add_observer(ClientOf::add_sender_channel::<C>);
            }
            NetworkDirection::ServerToClient => {
                self.app.add_observer(ClientOf::add_receiver_channel::<C>);
            }
            NetworkDirection::Bidirectional => {
                self.add_direction(NetworkDirection::ClientToServer);
                self.add_direction(NetworkDirection::ServerToClient);
            }
        }
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
