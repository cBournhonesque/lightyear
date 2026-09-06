//! Entity replication layer for lightyear, built on top of [`bevy_replicon`].
//!
//! This crate handles replicating ECS entities and their components across the
//! network. It wraps `bevy_replicon`'s low-level replication machinery and adds
//! lightyear-specific features: prediction/interpolation targets, network
//! visibility, hierarchy propagation, and pre-spawning.
//!
//! # Getting started
//!
//! Add [`Replicate`] to an entity to start replicating it. This automatically
//! inserts [`Replicating`]. On the server, you typically specify which clients
//! should receive the entity:
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
//! `PredictionTarget` is the send-side way to enable prediction for an entity;
//! the receiver can also insert [`Predicted`] directly on the received entity.
//! Each target uses a [`ReplicationMode`] to specify the set of recipients.
//! Remove [`Replicating`] to pause replication without despawning the remote
//! entity, then insert it again to resume.
//!
//! A [`ReplicationSender`] component must be present on the link entity
//! (the entity that represents the connection to a remote peer) to enable
//! outgoing replication through that link.
//!
//! ## Hierarchy propagation
//!
//! When an entity with [`Replicate`] has children (via `ChildOf`), those
//! children automatically receive a [`ReplicateLike`] component pointing back
//! to the root.
//! [`ReplicateLike`] ensures that the child entity copies the root's replication
//! behaviour (replication target, prediction target, visibility, rooms, etc.)
//! The child entity can always override any of these behaviors, for example by inserting
//! a [`Replicate`] component (in which case all other behaviour will still be propagated
//! via [`ReplicateLike`] except for [`Replicate`])
//!  Use [`DisableReplicateHierarchy`] on a child to opt out adding [`ReplicateLike`] automatically.
//!
//! You can also manually add [`ReplicateLike`] on any entity.
//!
//! Rooms and manual visibility are inherited from the root, but a member with
//! explicitly set rooms or visibility keeps them (tracked with
//! [`RoomsOverridden`](crate::visibility::room::RoomsOverridden) and
//! [`VisibilityOverridden`](crate::visibility::immediate::VisibilityOverridden));
//! remove the marker to re-inherit.
//!
//! ## Visibility
//!
//! [`VisibilityExt::gain_visibility`] and [`VisibilityExt::lose_visibility`]
//! let you dynamically show or hide an entity for a specific client. The default
//! [`VisibilityExt::lose_visibility`] behavior despawns the remote entity. Use
//! [`VisibilityExt::lose_visibility_retained`] to retain an entity the client has already seen, or
//! [`VisibilityExt::lose_visibility_always_present`] to guarantee an initial remote entity even
//! when it starts hidden. Both retaining policies pause updates while hidden.
//! Visibility changes propagate through [`ReplicateLikeChildren`] so that
//! hiding a parent also hides its replicated descendants.
//!
//! For interest management based on spatial regions, see [`RoomPlugin`].
//!
//! ## Control
//!
//! [`ControlledBy`] marks which link entity "owns" a replicated entity.
//!
//! ## Pre-spawning
//!
//! [`PreSpawned`] allows both client and server to spawn the same entity
//! independently, then match them via a deterministic hash. This enables
//! zero-latency predicted spawns (e.g. bullets, projectiles).
//! Sender-side [`PreSpawned::for_client`] scopes the matching signature to one
//! remote client without changing the entity's replication visibility.
//! Rollback and timeout bookkeeping lives in the world-global
//! [`prespawn::PreSpawnedReceiver`] resource.
//!
//! [`Replicate`]: crate::send::Replicate
//! [`Replicating`]: crate::send::Replicating
//! [`ReplicationTarget<()>`]: crate::send::ReplicationTarget
//! [`PredictionTarget`]: crate::send::PredictionTarget
//! [`InterpolationTarget`]: crate::send::InterpolationTarget
//! [`Predicted`]: lightyear_core::prediction::Predicted
//! [`ReplicationMode`]: crate::send::ReplicationMode
//! [`ReplicationSender`]: crate::send::ReplicationSender
//! [`ReplicateLike`]: crate::hierarchy::ReplicateLike
//! [`DisableReplicateHierarchy`]: crate::hierarchy::DisableReplicateHierarchy
//! [`ReplicateLikeChildren`]: crate::hierarchy::ReplicateLikeChildren
//! [`VisibilityExt::gain_visibility`]: crate::visibility::immediate::VisibilityExt::gain_visibility
//! [`VisibilityExt::lose_visibility`]: crate::visibility::immediate::VisibilityExt::lose_visibility
//! [`VisibilityExt::lose_visibility_retained`]: crate::visibility::immediate::VisibilityExt::lose_visibility_retained
//! [`VisibilityExt::lose_visibility_always_present`]: crate::visibility::immediate::VisibilityExt::lose_visibility_always_present
//! [`RoomPlugin`]: crate::visibility::room::RoomPlugin
//! [`ControlledBy`]: crate::control::ControlledBy
//! [`PreSpawned`]: crate::prespawn::PreSpawned
#![no_std]

extern crate alloc;
#[cfg(feature = "std")]
extern crate std;

use bevy_app::PluginGroupBuilder;
use bevy_app::prelude::Plugin;
use bevy_app::prelude::PluginGroup;
use bevy_ecs::prelude::SystemSet;

#[cfg(feature = "server")]
pub mod server;

pub mod channels;
pub mod checkpoint;
#[cfg(feature = "client")]
pub mod client;
pub mod control;
pub mod deferred_entity;
pub mod diff_history;
pub mod diffable;
pub mod hierarchy;
pub mod metadata;
pub mod prespawn;
pub mod receive;
pub mod registry;
pub mod send;

pub mod visibility;

mod impls;

pub mod prelude {
    pub use bevy_replicon::client::confirm_history::ConfirmHistory;
    pub use bevy_replicon::client::server_mutate_ticks::ServerMutateTicks;
    pub use bevy_replicon::prelude::Remote;
    pub use bevy_replicon::prelude::Remote as Replicated;
    #[cfg(feature = "server")]
    pub use bevy_replicon::server::{PriorityMap, ReplicatePriority};

    pub use crate::ReplicationSystems;
    pub use crate::checkpoint::ReplicationCheckpointMap;
    pub use crate::control::{Controlled, ControlledBy, ControlledSend, Lifetime};
    pub use crate::deferred_entity::DeferredEntityCommands;
    pub use crate::diff_history::HistoryDiffReceiver;
    pub use crate::hierarchy::{DisableReplicateHierarchy, ReplicateLike};
    pub use crate::metadata::{ReplicationMetadata, SenderMetadata};
    pub use crate::prespawn::PreSpawned;
    pub use crate::receive::{Persistent, ReplicationReceiver};
    pub use crate::send::{Replicate, ReplicatedFrom, Replicating, ReplicationSender};

    pub use crate::registry::ComponentRegistry;
    pub use crate::registry::TransformLinearInterpolation;
    pub use crate::registry::replication::{AppComponentExt, ComponentRegistrator};
    pub use crate::visibility::immediate::{
        NetworkVisibilityPlugin, VisibilityExt, VisibilityOverridden,
    };
    pub use crate::visibility::room::{RoomAllocator, RoomId, RoomPlugin, Rooms, RoomsOverridden};

    pub use crate::diffable::Diffable;

    #[cfg(feature = "interpolation")]
    pub use crate::send::{InterpolatedSend, InterpolationTarget};
    #[cfg(feature = "prediction")]
    pub use crate::send::{PredictedSend, PredictionTarget};

    #[cfg(feature = "client")]
    pub mod client {
        pub use bevy_replicon::prelude::Remote;
        pub use bevy_replicon::prelude::Remote as Replicated;
    }
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

/// Plugin that registers sender-side marker components (`PredictedSend`,
/// `InterpolatedSend`, `ControlledSend`) with Replicon on both client and
/// server. Their custom receive functions materialize the corresponding
/// receiver-local markers without putting those markers on authoritative
/// send-side entities.
struct SharedComponentRegistrationPlugin;

impl Plugin for SharedComponentRegistrationPlugin {
    fn build(&self, app: &mut bevy_app::prelude::App) {
        use bevy_ecs::prelude::ChildOf;
        use bevy_replicon::prelude::{AppMarkerExt, AppRuleExt, RuleFns};
        app.init_resource::<registry::ComponentRegistry>();
        // The order of app.replicate() calls must be identical on client and server.
        // These sender markers are translated into receiver-local markers by
        // their custom receive functions.
        #[cfg(feature = "prediction")]
        app.replicate::<send::PredictedSend>()
            .set_receive_fns::<send::PredictedSend>(send::write_predicted, send::remove_predicted);
        #[cfg(feature = "interpolation")]
        app.replicate::<send::InterpolatedSend>()
            .set_receive_fns::<send::InterpolatedSend>(
                send::write_interpolated,
                send::remove_interpolated,
            );
        app.replicate::<control::ControlledSend>()
            .set_receive_fns::<control::ControlledSend>(
                control::write_controlled,
                control::remove_controlled,
            );
        // ChildOf is registered for replication in HierarchySendPlugin (server-only),
        // but must also be registered on the client so FnsIds match.
        app.replicate_with(RuleFns::new(
            hierarchy::serialize_child_of,
            hierarchy::deserialize_child_of,
        ))
        .set_receive_fns::<ChildOf>(hierarchy::write_child_of, hierarchy::remove_child_of);

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
        // We enable this Replicon setting when prediction/interpolation is active:
        // - replication mutations are sent every RepliconTick, even if there were 0 mutations.
        //   This avoids situations where a client mispredicted something, and the sender does
        //   not send any further corrections because nothing changed.
        // - it adds a `ServerMutateTicks` resource on the receiver that keeps track of the ticks
        //   where the receiver received any messages.
        app.add_plugins(bevy_replicon::server::ServerPlugin {
            tick_schedule: None,
            track_mutate_messages: cfg!(any(feature = "prediction", feature = "interpolation")),
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
        app.add_systems(
            bevy_app::prelude::PreUpdate,
            hierarchy::resolve_pending_child_of,
        );
    }
}
