use bevy::prelude::SystemSet;

/// System sets related to Replication
#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone, Copy)]
pub enum ReplicationSet {
    /// System Set to gather all the replication updates to send
    SendEntityUpdates,
    SendComponentUpdates,
}

#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone, Copy)]
pub enum MainSet {
    /// Systems that receive data (buffer any data received from transport, and read
    /// data from the buffers)
    ///
    /// Runs in `PreUpdate`.
    Receive,
    ReceiveFlush,

    /// Runs once per frame, update sync (client only)
    Sync,
    /// Runs once per frame, clears events (server only)
    ClearEvents,

    /// Systems that send data (buffer any data to be sent, and send any buffered packets)
    ///
    /// Runs in `PostUpdate`.
    SendPackets,
    /// System to encompass all send-related systems
    Send,
}

#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone, Copy)]
pub enum FixedUpdateSet {
    /// System that runs at the very start of the FixedUpdate schedule to increment the ticks
    TickUpdate,
    /// Main loop (with physics, game logic) during FixedUpdate
    Main,
    MainFlush,
}
