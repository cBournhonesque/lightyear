use bevy_ecs::schedule::SystemSet;

/// Set with replication and event systems related to client.
#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone, Copy)]
pub enum ClientSet {
    /// Systems that receive data (buffer any data received from transport, and read
    /// data from the buffers)
    ///
    /// Runs in `PreUpdate`.
    Receive,
    /// Systems that send data (buffer any data to be sent, and send any buffered packets)
    ///
    /// Runs in `PostUpdate`.
    Send,
}
