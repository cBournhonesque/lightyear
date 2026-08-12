use crate::network_topology::NetworkingMetadata;
use bevy_ecs::system::Res;

/// Returns true for a connected conventional client topology.
///
/// Direct peers are reported by [`is_p2p`], not as conventional clients, even though every
/// [`crate::p2p::P2P`] Link carries the internal [`crate::client::Client`] role marker.
pub fn is_client(metadata: Res<NetworkingMetadata>) -> bool {
    metadata.mode.is_client()
}

/// Returns true for a started server, including the server side of a ready host-client topology.
pub fn is_server(metadata: Res<NetworkingMetadata>) -> bool {
    metadata.mode.is_server()
}

/// Returns true for a started server without a connected in-process client.
pub fn is_headless_server(metadata: Res<NetworkingMetadata>) -> bool {
    metadata.mode.is_headless_server()
}

/// Returns true if we are running in host-server mode, i.e. the server is acting as a client
/// (in which case we can disable the networking/prediction/interpolation systems on the client)
///
/// We are in host-server mode when both sides form a ready cached host-client topology.
pub fn is_host_server(metadata: Res<NetworkingMetadata>) -> bool {
    metadata.mode.is_host_server()
}

/// Returns true after the P2PStarted event has been triggered
pub fn is_p2p(metadata: Res<NetworkingMetadata>) -> bool {
    metadata.mode.is_p2p()
}
