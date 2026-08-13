use crate::client::Client;
use bevy_ecs::prelude::*;
use bevy_reflect::Reflect;

/// Participation state of a direct peer Link in a P2P session.
///
/// A P2P Link remains a [`Client`] Link so that it can reuse the existing connection,
/// messaging, input, and prediction pipelines. The component is immutable: replace it to move a
/// Link between states so that the cached network topology observes every transition.
#[derive(Component, Default, Debug, Clone, Copy, PartialEq, Eq, Reflect)]
#[component(immutable)]
#[require(Client)]
pub enum P2P {
    /// A declared P2P Link that is not part of a start barrier or running session.
    ///
    /// Inactive Links do not activate a P2P [`NetworkTopology`](crate::network_topology::NetworkTopology).
    #[default]
    Inactive,
    /// A Link frozen into the current start barrier.
    ///
    /// Candidate Links are intentionally not exposed through the cached
    /// [`NetworkTopology`](crate::network_topology::NetworkTopology). Systems that participate in
    /// startup synchronization should query this component directly.
    Candidate,
    /// A Link that has crossed the start barrier. While connected, it is exposed through
    /// [`NetworkTopology::P2P`](crate::network_topology::NetworkTopology::P2P).
    Joined,
}
