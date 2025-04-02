use crate::direction::NetworkDirection;
use bevy::prelude::{Component, Event, OnAdd, Query, Res, Trigger};
use lightyear_messages::receive::MessageReceiver;
use lightyear_messages::registry::MessageRegistration;
use lightyear_messages::send::MessageSender;
use lightyear_messages::{Message, MessageManager};
use lightyear_transport::channel::registry::ChannelRegistration;
use lightyear_transport::channel::Channel;
use lightyear_transport::prelude::{ChannelRegistry, Transport};

// TODO: should this be a component?
#[derive(Debug)]
pub enum ConnectionState {
    Disconnected { reason: Option<ConnectionError> },
    Connecting,
    Connected,
}


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


/// Marker component to identify this entity as a Client
#[derive(Component, Default)]
#[require(MessageManager)]
pub struct Client;

impl Client {
    pub(crate) fn add_sender_channel<C: Channel>(trigger: Trigger<OnAdd, Client>, mut query: Query<&mut Transport>, registry: Res<ChannelRegistry>) {
        if let Ok(mut transport) = query.get_mut(trigger.target()) {
            transport.add_sender_from_registry::<C>(&registry)
        }
    }

    pub(crate) fn add_receiver_channel<C: Channel>(trigger: Trigger<OnAdd, Client>, mut query: Query<&mut Transport>, registry: Res<ChannelRegistry>) {
        if let Ok(mut transport) = query.get_mut(trigger.target()) {
            transport.add_receiver_from_registry::<C>(&registry)
        }
    }
}

/// Trigger to connect the client
#[derive(Event)]
pub struct Connect;

/// Trigger to disconnect the client
#[derive(Event)]
pub struct Disconnect;

// TODO: on_add: remove Connecting/Disconnected
#[derive(Component, Default, Debug)]
pub struct Connected;

// TODO: add automatic disconnection for entities that are Connecting for too long
#[derive(Component, Event, Default, Debug)]
pub struct Connecting;

#[derive(Component, Default, Debug)]
pub struct Disconnected;


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
                self.app.register_required_components::<Client, MessageSender<M>>();
            }
            NetworkDirection::ServerToClient => {
                self.app.register_required_components::<Client, MessageReceiver<M>>();
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
                self.app.add_observer(Client::add_sender_channel::<C>);
            }
            NetworkDirection::ServerToClient => {
                self.app.add_observer(Client::add_receiver_channel::<C>);
            }
            NetworkDirection::Bidirectional => {
                self.add_direction(NetworkDirection::ClientToServer);
                self.add_direction(NetworkDirection::ServerToClient);
            }
        }
    }
}


#[cfg(test)]
mod tests {
    #[test]
    fn test_connection() {

    }


}