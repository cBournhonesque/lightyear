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
    pub use crate::authority::{AuthorityBroker, GiveAuthority, HasAuthority};
    pub use crate::checkpoint::ReplicationCheckpointMap;
    pub use crate::control::{Controlled, ControlledBy, Lifetime};
    pub use crate::hierarchy::{DisableReplicateHierarchy, ReplicateLike};
    pub use crate::metadata::{ReplicationMetadata, SenderMetadata};
    pub use crate::prespawn::PreSpawned;
    pub use crate::receive::ReplicationReceiver;
    pub use crate::send::{Replicate, ReplicatedFrom, ReplicationSender};

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
