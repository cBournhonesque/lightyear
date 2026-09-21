/*! # Lightyear Connection

Connection identity, admission, and lifecycle on top of native links, for client/server,
in-process host-client, and explicit P2P topologies. A bare link or default/debug
connection does not imply authentication or negotiated identity.

## Identity and routing scope

Keep application account/player identity, numeric routing aliases
([`PeerId`](lightyear_core::id::PeerId)), and full transport-owned native identities
separate. Native addresses and mutable dial context are not stable identity, and a
routing alias is neither a dial target nor proof of authentication.

[`LocalId`](lightyear_core::id::LocalId) and [`RemoteId`](lightyear_core::id::RemoteId)
are final when [`Connected`](client::Connected) is exposed. Admission registers the
remote alias in [`NetworkingMetadata::peer_map`](network_topology::NetworkingMetadata::peer_map)
before notifying observers. This registry spans the entire World, not one endpoint
or backend.

`PeerId::SERVER` (zero) is reserved for the appropriate client/server role. A started
local server endpoint reserves that global route, as does a conventional client's
remote-server connection. A competing local server endpoint or remote server fails
rather than replacing the owner. Independent client/server scopes need separate
Worlds; explicit P2P uses nonzero peer aliases and its own discovery/roster contract.
[`HostClient`](host::HostClient) is an in-process path, not Crossbeam, and also uses
a nonzero alias.

## Backend and transport obligations

* Configure one backend before connecting. Explicit backend components require
  [`NoDefaultConnection`](admission::NoDefaultConnection) to suppress default admission
  on the link or an endpoint's children. Otherwise supported transports use the
  default/debug path. Competing backends and live backend switching are unsupported;
  there is no backend ownership or conflict arbitration.
* Retain full native identity and dialing facts in transport-owned components.
  Resolve both routing identities and complete any required authentication or P2P
  preadmission discovery before publishing the connection.
* For default/debug admission, install
  [`DefaultConnectionPlugin`](admission::DefaultConnectionPlugin) and call
  [`apply_default_ids`](admission::apply_default_ids) with native fallback candidates.
  It respects requested IDs, roles, and
  [`NoDefaultConnection`](admission::NoDefaultConnection) on the link or endpoint.
  Opt-out leaves a byte link without implicit connection admission.
* Explicit backends call [`admit`](admission::admit) with final local and remote IDs
  and an additional component bundle (`()` when none is needed), and check the result.
  The bundle is inserted atomically with `Connected`, after IDs and routing are registered.
  Never insert `Connected` directly, mutate admitted IDs, or overwrite a registry route.
  An identical repeated admission is allowed; rebinding a connected entity is rejected.
* Server backends call [`start_server`](server::start_server) rather than inserting
  [`Started`](server::Started) directly. Conflicting server routes are rejected before
  readiness observers run.
* Use [`reject`](admission::reject) for admission failure. It records
  [`Disconnected`](client::Disconnected) and triggers native unlinking. Implement
  native close handling and normal disconnect transitions on link loss. Preserve
  `RemoteId` through teardown: the internal `Connected` removal hook and
  `Disconnected` hook release only a binding still owned by that entity. Despawn
  runs removal hooks too; stale teardown must never remove another owner's route.

## Collisions and agreement

Authenticated/netcode, P2P, and explicitly requested duplicate remote aliases fail
across all endpoints and backends. Only an unrequested non-P2P default/debug remote
alias other than `SERVER` may fall back to a free local label after collision; the
incumbent is never renamed. Host clients allocate a free nonzero alias if none was
requested; explicit host IDs must agree and pass ordinary collision checks.

Default/debug admission does not authenticate or negotiate these labels. NAT and
local-only links may produce different labels at either end. P2P lobby admission
allows only its bounded, own-only discovery announcement before publishing IDs;
admitted senders cannot rename themselves, and third-party discovery cannot replace
a direct binding. Collision rejection does not establish globally consistent
claimant selection: the existing shared-roster barrier remains required before gameplay.
*/
#![no_std]

extern crate alloc;
extern crate core;

use bevy_app::{App, Plugin};
use bevy_ecs::schedule::SystemSet;

pub mod admission;
pub mod client;

pub mod server;

pub mod direction;
pub mod network_target;

pub mod client_of;
#[allow(unused)]
pub mod identity;
pub mod shared;

pub mod host;
pub mod network_topology;
pub mod p2p;

/// System sets for connection-related logic.
/// These are used to order systems that handle receiving and sending packets.
#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone, Copy)]
pub enum ConnectionSystems {
    // PRE UPDATE
    /// Receive bytes from the Link, process them as Packets and buffer them into the Transport
    Receive,

    // PostUpdate
    /// Flush the messages that were buffered into the Transport, process them as Packets and
    /// buffer them to the Link
    Send,
}

pub mod prelude {
    pub use crate::ConnectionSystems;
    pub use crate::admission::{
        DefaultConnectionPlugin, NoDefaultConnection, admit, apply_default_ids,
        is_default_connection,
    };
    pub use crate::direction::NetworkDirection;
    pub use crate::identity::{is_client, is_p2p};
    pub use crate::network_target::NetworkTarget;
    pub use crate::network_topology::{
        NetworkTopology, NetworkTopologyError, NetworkTopologySystems, NetworkingMetadata,
    };
    // we also export these types at the top level for easier access
    pub use crate::client::{
        Client, ClientState, Connect, Connected, Connecting, ConnectionError, Disconnect,
        Disconnected, DisconnectedReason,
    };
    pub use crate::p2p::P2P;

    #[cfg(feature = "client")]
    pub mod client {
        pub use crate::client::{
            Client, ClientState, Connect, Connected, Connecting, ConnectionError, Disconnect,
            Disconnected, DisconnectedReason,
        };
    }

    #[cfg(feature = "server")]
    pub mod server {
        pub use crate::client_of::ClientOf;
        pub use crate::identity::{is_headless_server, is_host_server, is_server};
        pub use crate::server::{
            ConnectionError, Start, Started, Starting, Stop, Stopped, start_server,
        };
    }
}

/// Plugin to handle the connection logic.
/// Registers relevant types.
pub struct ConnectionPlugin;

impl Plugin for ConnectionPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(network_topology::NetworkTopologyPlugin);
    }
}
