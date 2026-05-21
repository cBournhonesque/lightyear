//! Direction markers for client/server traffic.

/// Direction in which a channel, message, or replication flow can send data.
///
/// This is metadata for higher-level APIs; it does not itself enforce permissions on a
/// [`Link`](lightyear_link::Link).
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum NetworkDirection {
    /// Data flows from a client entity to the server entity.
    ClientToServer,
    /// Data flows from the server entity to one or more client entities.
    ServerToClient,
    /// Data may flow in both client-to-server and server-to-client directions.
    Bidirectional,
}
