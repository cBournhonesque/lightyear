use bevy::prelude::{Component, Event, OnAdd, Query, Res, Trigger};
use lightyear_messages::MessageManager;
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
#[derive(Component)]
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
#[derive(Component)]
pub struct Connected;

#[derive(Component)]
pub struct Connecting;



#[derive(Component)]
pub struct Disconnected;


#[cfg(test)]
mod tests {
    #[test]
    fn test_connection() {

    }


}