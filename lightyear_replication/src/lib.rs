//! Entity replication layer for lightyear, built on top of [`bevy_replicon`].
//!
//! This crate handles replicating ECS entities and their components across the
//! network. It wraps `bevy_replicon`'s low-level replication machinery and adds
//! lightyear-specific features: prediction/interpolation targets, network
//! visibility, authority, hierarchy propagation, and pre-spawning.
//!
//! # Getting started
//!
//! Add [`Replicate`] to an entity to start replicating it. On the server, you
//! typically specify which clients should receive the entity:
//!
//! ```rust,ignore
//! commands.spawn((
//!     Replicate::to_clients(NetworkTarget::All),
//!     PredictionTarget::to_clients(NetworkTarget::Single(client_id)),
//!     InterpolationTarget::to_clients(NetworkTarget::AllExceptSingle(client_id)),
//!     MyComponent(42),
//! ));
//! ```
//!
//! # Key concepts
//!
//! ## Replication targets
//!
//! [`Replicate`] (alias for [`ReplicationTarget<()>`]) controls which peers
//! receive an entity. [`PredictionTarget`] and [`InterpolationTarget`] further
//! control which clients run prediction or interpolation for that entity.
//! Each target uses a [`ReplicationMode`] to specify the set of recipients.
//!
//! A [`ReplicationSender`] component must be present on the link entity
//! (the entity that represents the connection to a remote peer) to enable
//! outgoing replication through that link.
//!
//! ## Hierarchy propagation
//!
//! When an entity with [`Replicate`] has children (via `ChildOf`), those
//! children automatically receive a [`ReplicateLike`] component pointing back
//! to the root. This clones the root's replication configuration onto the
//! child so the entire hierarchy replicates with the same visibility rules.
//! Use [`DisableReplicateHierarchy`] on a child to opt out.
//!
//! You can also manually add [`ReplicateLike`] on any entity.
//!
//! ## Visibility
//!
//! [`VisibilityExt::gain_visibility`] and [`VisibilityExt::lose_visibility`]
//! let you dynamically show or hide an entity for a specific client.
//! Visibility changes propagate through [`ReplicateLikeChildren`] so that
//! hiding a parent also hides its replicated descendants.
//!
//! For interest management based on spatial regions, see [`RoomPlugin`].
//!
//! ## Authority and control
//!
//! [`ControlledBy`] marks which link entity "owns" a replicated entity.
//! [`HasAuthority`] indicates the local peer currently has authority.
//! See [`AuthorityBroker`] for tracking authority across the replication
//! hierarchy.
//!
//! Authority is currently not working since replicon only supports server to client
//! replication.
//!
//! ## Pre-spawning
//!
//! [`PreSpawned`] allows both client and server to spawn the same entity
//! independently, then match them via a deterministic hash. This enables
//! zero-latency predicted spawns (e.g. bullets, projectiles).
//!
//! [`Replicate`]: crate::send::Replicate
//! [`ReplicationTarget<()>`]: crate::send::ReplicationTarget
//! [`PredictionTarget`]: crate::send::PredictionTarget
//! [`InterpolationTarget`]: crate::send::InterpolationTarget
//! [`ReplicationMode`]: crate::send::ReplicationMode
//! [`ReplicationSender`]: crate::send::ReplicationSender
//! [`ReplicateLike`]: crate::hierarchy::ReplicateLike
//! [`DisableReplicateHierarchy`]: crate::hierarchy::DisableReplicateHierarchy
//! [`ReplicateLikeChildren`]: crate::hierarchy::ReplicateLikeChildren
//! [`VisibilityExt::gain_visibility`]: crate::visibility::immediate::VisibilityExt::gain_visibility
//! [`VisibilityExt::lose_visibility`]: crate::visibility::immediate::VisibilityExt::lose_visibility
//! [`RoomPlugin`]: crate::visibility::room::RoomPlugin
//! [`ControlledBy`]: crate::control::ControlledBy
//! [`HasAuthority`]: crate::authority::HasAuthority
//! [`AuthorityBroker`]: crate::authority::AuthorityBroker
//! [`PreSpawned`]: crate::prespawn::PreSpawned
#![no_std]

extern crate alloc;
#[cfg(feature = "std")]
extern crate std;

use bevy_app::PluginGroupBuilder;
#[cfg(feature = "client")]
use bevy_app::prelude::Plugin;
use bevy_app::prelude::PluginGroup;
use bevy_ecs::prelude::SystemSet;

#[cfg(feature = "server")]
pub mod server;

pub mod authority;
pub mod channels;
pub mod checkpoint;
#[cfg(feature = "client")]
pub mod client;
pub mod control;
pub mod hierarchy;
pub mod metadata;
pub mod prespawn;
pub mod receive;
pub mod registry;
pub mod send;

pub mod visibility;

#[cfg(feature = "delta")]
pub mod delta;

#[cfg(feature = "delta")]
mod impls;

pub mod prelude {
    pub use bevy_replicon::client::confirm_history::ConfirmHistory;
    pub use bevy_replicon::client::server_mutate_ticks::ServerMutateTicks;
    pub use bevy_replicon::prelude::Replicated;

    pub use crate::ReplicationSystems;
    pub use crate::authority::{AuthorityBroker, GiveAuthority, HasAuthority, RequestAuthority};
    pub use crate::checkpoint::ReplicationCheckpointMap;
    pub use crate::control::{Controlled, ControlledBy, Lifetime};
    pub use crate::hierarchy::{DisableReplicateHierarchy, ReplicateLike};
    pub use crate::metadata::{ReplicationMetadata, SenderMetadata};
    pub use crate::prespawn::PreSpawned;
    pub use crate::receive::ReplicationReceiver;
    pub use crate::send::{Replicate, ReplicatedFrom, ReplicationSender, ReplicationState};

    pub use crate::registry::ComponentRegistry;
    pub use crate::registry::TransformLinearInterpolation;
    pub use crate::registry::replication::AppComponentExt;
    pub use crate::visibility::immediate::{NetworkVisibilityPlugin, VisibilityExt};
    pub use crate::visibility::room::{RoomAllocator, RoomId, RoomPlugin, Rooms};

    #[cfg(feature = "delta")]
    pub use crate::delta::{DeltaManager, Diffable};

    #[cfg(feature = "interpolation")]
    pub use crate::send::InterpolationTarget;
    #[cfg(feature = "prediction")]
    pub use crate::send::PredictionTarget;
}

#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone, Copy)]
pub enum ReplicationSystems {
    // PreUpdate
    /// Receive replication messages and apply them to the World
    Receive,

    // PostUpdate
    /// Flush the messages buffered in the Link to the io
    Send,
}

/// Plugin that registers replicated marker components (`Predicted`, `Interpolated`, `Controlled`)
/// with replicon on both client and server. This ensures the component ID registry matches
/// on both sides, which is required for correct deserialization.
struct SharedComponentRegistrationPlugin;

impl bevy_app::prelude::Plugin for SharedComponentRegistrationPlugin {
    fn build(&self, app: &mut bevy_app::prelude::App) {
        use bevy_replicon::prelude::AppRuleExt;
        // The order of app.replicate() calls must be identical on client and server.
        // These marker components are sent from server to client as part of entity replication.
        #[cfg(feature = "prediction")]
        app.replicate::<lightyear_core::prediction::Predicted>();
        #[cfg(feature = "interpolation")]
        app.replicate::<lightyear_core::interpolation::Interpolated>();
        app.replicate::<control::Controlled>();
        // ChildOf is registered for replication in HierarchySendPlugin (server-only),
        // but must also be registered on the client so FnsIds match.
        app.replicate::<bevy_ecs::prelude::ChildOf>();

        // ServerMutateTicks is normally only initialized by bevy_replicon's ClientPlugin,
        // but prediction systems on server-only builds also reference it. Init it here
        // so it's always available (defaults to empty/harmless state).
        #[cfg(any(feature = "prediction", feature = "interpolation"))]
        app.init_resource::<bevy_replicon::client::server_mutate_ticks::ServerMutateTicks>();
        app.init_resource::<checkpoint::ReplicationCheckpointMap>();
    }
}

pub struct LightyearRepliconBackend;

impl PluginGroup for LightyearRepliconBackend {
    fn build(self) -> PluginGroupBuilder {
        let mut group = PluginGroupBuilder::start::<Self>();

        group = group.add(bevy_replicon::shared::RepliconSharedPlugin {
            auth_method: bevy_replicon::shared::AuthMethod::None,
        });
        group = group.add(channels::RepliconChannelRegistrationPlugin);
        group = group.add(metadata::MetadataPlugin);
        group = group.add(prespawn::PreSpawnedPlugin);
        // Register shared marker components before server/client-specific plugins,
        // so that both sides have matching replicon component IDs.
        group = group.add(SharedComponentRegistrationPlugin);

        group
    }
}

#[cfg(feature = "server")]
pub struct LightyearRepliconServerBackend;

#[cfg(feature = "server")]
impl Plugin for LightyearRepliconServerBackend {
    fn build(&self, app: &mut bevy_app::prelude::App) {
        app.add_plugins(bevy_replicon::server::ServerPlugin {
            tick_schedule: None,
            ..Default::default()
        });
        app.add_plugins(server::RepliconServerPlugin);
        app.add_plugins(send::SendPlugin);
        app.add_plugins(control::ControlPlugin);
        app.add_plugins(hierarchy::HierarchyPlugin);
        app.add_plugins(hierarchy::HierarchySendPlugin::<bevy_ecs::prelude::ChildOf>::default());
        app.add_plugins(visibility::immediate::NetworkVisibilityPlugin);
        app.add_observer(send::handle_new_client_visibility);
    }
}

#[cfg(feature = "client")]
pub struct LightyearRepliconClientBackend;

#[cfg(feature = "client")]
impl Plugin for LightyearRepliconClientBackend {
    fn build(&self, app: &mut bevy_app::prelude::App) {
        app.add_plugins(bevy_replicon::client::ClientPlugin);
        app.add_plugins(client::RepliconClientPlugin);
    }
}
