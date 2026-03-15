#![no_std]

extern crate alloc;
#[cfg(feature = "std")]
extern crate std;

use bevy_app::PluginGroupBuilder;
use bevy_app::prelude::PluginGroup;
use bevy_ecs::prelude::SystemSet;

#[cfg(feature = "server")]
pub mod server;

#[cfg(feature = "client")]
pub mod client;
pub mod channels;
pub mod send;
pub mod registry;
pub mod metadata;
pub mod authority;
pub mod control;
pub mod receive;
pub mod prespawn;
pub mod hierarchy;

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
    pub use crate::metadata::{ReplicationMetadata, SenderMetadata};
    pub use crate::authority::{AuthorityBroker, GiveAuthority, HasAuthority};
    pub use crate::control::{Controlled, ControlledBy, Lifetime};
    pub use crate::hierarchy::{DisableReplicateHierarchy, ReplicateLike};
    pub use crate::prespawn::PreSpawned;
    pub use crate::receive::ReplicationReceiver;
    pub use crate::send::{Replicate, ReplicatedFrom, ReplicationSender};

    pub use crate::visibility::room::{RoomAllocator, RoomPlugin, Rooms, RoomId};
    pub use crate::visibility::immediate::{NetworkVisibilityPlugin, VisibilityExt};
    pub use crate::registry::replication::AppComponentExt;
    pub use crate::registry::ComponentRegistry;
    pub use crate::registry::TransformLinearInterpolation;

    #[cfg(feature = "delta")]
    pub use crate::delta::{Diffable, DeltaManager};

    #[cfg(feature = "prediction")]
    pub use crate::send::PredictionTarget;
    #[cfg(feature = "interpolation")]
    pub use crate::send::InterpolationTarget;
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

        // ServerMutateTicks is normally only initialized by bevy_replicon's ClientPlugin,
        // but prediction systems on server-only builds also reference it. Init it here
        // so it's always available (defaults to empty/harmless state).
        #[cfg(any(feature = "prediction", feature = "interpolation"))]
        app.init_resource::<bevy_replicon::client::server_mutate_ticks::ServerMutateTicks>();
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

        #[cfg(feature = "server")]
        {
            let mut server_plugin = bevy_replicon::server::ServerPlugin::default();
            server_plugin.tick_schedule = None;
            group = group.add(server_plugin);
            group = group.add(server::RepliconServerPlugin);

            group = group.add(send::SendPlugin);
            group = group.add(control::ControlPlugin);
            group = group.add(hierarchy::HierarchyPlugin);
            group = group.add(hierarchy::HierarchySendPlugin::<bevy_ecs::prelude::ChildOf>::default());
            group = group.add(visibility::immediate::NetworkVisibilityPlugin);

        }

        #[cfg(feature = "client")]
        {
            group = group.add(bevy_replicon::client::ClientPlugin);
            group = group.add(client::RepliconClientPlugin);
        }

        group
    }
}
