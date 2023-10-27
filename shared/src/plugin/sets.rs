use bevy::prelude::SystemSet;

/// System sets related to Replication
#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone, Copy)]
pub enum ReplicationSet {
    /// System Set to gather all the replication updates to send
    SendEntityUpdates,

    SendComponentUpdates,
}
