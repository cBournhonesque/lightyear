//! Late-join catch-up for deterministic replication.
//!
//! # Problem
//!
//! In deterministic replication the server and all clients run the same
//! simulation driven only by inputs. For an already-connected client, the
//! initial state of an entity is established once (via `replicate_once` of
//! physics components at entity spawn) and then every peer simulates forward
//! from that point using inputs that are rebroadcast by the server.
//!
//! When a new client joins mid-game, that initial snapshot is already in the
//! past. Simply `replicate_once`-ing the *current* physics state would not
//! help because the new client does not yet have the remote inputs needed
//! to simulate forward from the snapshot tick.
//!
//! # Approach (client-driven, bundled)
//!
//! Information flows from the client — which actually knows what it has —
//! back to the server:
//!
//! 1. At join, the server replicates "structural" data for existing entities
//!    (markers like `PlayerId`, `DeterministicPredicted`) and starts
//!    rebroadcasting inputs for those entities. Physics components
//!    registered via `AppCatchUpExt::register_catchup` are
//!    **hidden by default** via a replicon per-component visibility filter
//!    until each client requests a catch-up snapshot.
//!
//! 2. On the client, catch-up entities are replicated with the `CatchUpGated`
//!    marker component. Once the `InputTimeline` is synced and we have received
//!    inputs from all clients, the plugin
//!    sends `CatchUpRequest` with that `input_safe_tick`.
//!
//! 3. On the server, the `CatchUpRequest` handler buffers the client's
//!    advertised input-safe tick until the authoritative server tick has moved
//!    past it. It then inserts `HasCaughtUp` on the client's link entity and
//!    sends a replicated `CatchUpSnapshotReady` event with the authoritative
//!    rollback tick and the Replicon checkpoint that contains the reveal. If
//!    the server determines that no snapshot is needed, it still sends
//!    `CatchUpSnapshotReady` with both ticks set to `u32::MAX`; the client
//!    treats that as a server-authoritative skip.
//!
//! 4. The client waits until Replicon's mutate completion state reports that
//!    the accepted Replicon checkpoint is fully confirmed. At that point all
//!    mutation messages for the catch-up reveal have landed, so the plugin
//!    emits `CatchUpSnapshotReady` for local-only setup and schedules one
//!    forced rollback to the accepted server tick. If that checkpoint becomes
//!    too old for the configured rollback window before it is usable, the
//!    client logs and disconnects; that should not happen in a healthy
//!    catch-up flow.
//!
//! 5. `HasCaughtUp` is not removed. After the initial catch-up, the rest of
//!    the simulation is deterministic, so later catch-up-gated components are
//!    replicated normally to that client.
//!
//! # Why per-component visibility, not entity-level
//!
//! If we hid the whole entity, the client would not know the entity
//! exists and could not send the catch-up request (nor receive rebroadcast
//! inputs). By keeping the entity visible but hiding only the physics
//! components, the client sees the entity + marker components + rebroadcast
//! inputs and can request the bundled snapshot.

#[cfg(feature = "client")]
mod client;
#[cfg(feature = "server")]
mod server;
mod shared;

#[cfg(feature = "client")]
pub use client::{CatchUpClientTimeout, CatchUpManager};
pub use shared::{
    AppCatchUpExt, CatchUpComponentScope, CatchUpRegistry, CatchUpRequest, CatchUpSnapshotReady,
    CatchUpSystems, HasCaughtUp,
};

use bevy_app::{App, Plugin};
use bevy_replicon::shared::{AuthMethod, RepliconSharedPlugin};
use lightyear_connection::direction::NetworkDirection;
use lightyear_inputs::input_message::ActionStateSequence;
use lightyear_messages::prelude::{AppMessageExt, AppTriggerExt};

/// Re-export of [`lightyear_prediction::rollback::CatchUpGated`]
/// so user code can stay in the catch-up vocabulary.
///
/// This is a **per-entity marker component** (not a resource). The late-join
/// plugin inserts it on catch-up-gated client entities while they are
/// expecting the bundled snapshot, and removes it once the forced rollback is
/// scheduled.
pub use lightyear_prediction::rollback::CatchUpGated;
use lightyear_replication::metadata::MetadataChannel;
use lightyear_replication::registry::replication::AppComponentExt;
use lightyear_transport::prelude::{AppChannelExt, ChannelMode, ChannelSettings, ReliableSettings};

use crate::mode::CatchUpMode;

/// Plugin that wires up the late-join catch-up machinery.
///
/// Clients send [`CatchUpRequest`] once they're synced, have at least one
/// catch-up-gated entity awaiting a snapshot, and have registered input
/// buffers covering a safe replay tick. The server accepts by inserting
/// [`HasCaughtUp`] and sending a replicated [`CatchUpSnapshotReady`] event.
/// The client waits for the accepted Replicon checkpoint to be fully
/// confirmed, re-triggers [`CatchUpSnapshotReady`] locally for activation
/// observers, and then drives the forced rollback. The [`HasCaughtUp`] marker
/// is kept after success.
pub struct LateJoinCatchUpPlugin;

impl Default for LateJoinCatchUpPlugin {
    fn default() -> Self {
        Self
    }
}

impl AppCatchUpExt for App {
    fn register_catchup<T, S>(&mut self) -> &mut Self
    where
        T: CatchUpComponentScope + Send + Sync + 'static,
        S: ActionStateSequence,
    {
        #[cfg(feature = "server")]
        server::register_catchup::<T>(self);

        self
    }
}

impl Plugin for LateJoinCatchUpPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<RepliconSharedPlugin>() {
            app.add_plugins(RepliconSharedPlugin {
                auth_method: AuthMethod::None,
            });
        }

        app.add_channel::<MetadataChannel>(ChannelSettings {
            mode: ChannelMode::UnorderedReliable(ReliableSettings::default()),
            ..Default::default()
        })
        .add_direction(NetworkDirection::Bidirectional);
        if !app.is_message_registered::<CatchUpRequest>() {
            app.register_message::<CatchUpRequest>()
                .add_direction(NetworkDirection::ClientToServer);
        }
        app.register_event::<CatchUpSnapshotReady>()
            .add_direction(NetworkDirection::ServerToClient);
        app.init_resource::<CatchUpRegistry>();
        app.init_resource::<CatchUpMode>();
        app.component::<CatchUpGated>().replicate_once();

        #[cfg(feature = "client")]
        client::build(app);
    }

    fn cleanup(&self, app: &mut App) {
        #[cfg(feature = "server")]
        server::build(app);
    }
}
