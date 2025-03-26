use bevy::prelude::Event;

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



/// A dummy connection plugin that takes payloads directly from the Link
/// to the Transport without any processing
pub struct PassthroughClientPlugin;

#[derive(Event)]
pub struct ConnectTrigger;

#[derive(Event)]
pub struct DisconnectTrigger;