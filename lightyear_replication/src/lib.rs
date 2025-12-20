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
pub mod send;
pub mod registry;
pub mod metadata;
pub mod authority;
pub mod control;
pub mod receive;
pub mod prespawn;

#[cfg(feature = "delta")]
pub mod delta;

pub mod prelude {
    pub use bevy_replicon::client::confirm_history::ConfirmHistory;
    pub use bevy_replicon::client::server_mutate_ticks::ServerMutateTicks;
    #[cfg(feature = "server")]
    pub use crate::server::ReplicationMetadata;

    pub use crate::ReplicationSystems;
    pub use crate::control::{Controlled, ControlledBy};
    pub use crate::prespawn::PreSpawned;
    pub use crate::receive::Confirmed;
    pub use crate::send::{Replicate, ReplicationSender};
    pub use crate::registry::replication::AppComponentExt;

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

pub struct LightyearRepliconBackend;

impl PluginGroup for LightyearRepliconBackend {
    fn build(self) -> PluginGroupBuilder {
        let mut group = PluginGroupBuilder::start::<Self>();

        group = group.add(bevy_replicon::shared::RepliconSharedPlugin::default());

        #[cfg(feature = "server")]
        {
            let mut server_plugin = bevy_replicon::server::ServerPlugin::default();
            server_plugin.tick_schedule = None;
            group = group.add(server_plugin);
            group = group.add(server::RepliconServerPlugin);


            // TODO: add this independently from client or server. This should be enabled on the sender side
            group = group.add(send::SendPlugin);
            group = group.add(control::ControlPlugin);
            group = group.add(metadata::MetadataPlugin);
        }

        #[cfg(feature = "client")]
        {
            group = group.add(bevy_replicon::client::ClientPlugin);
            group = group.add(client::RepliconClientPlugin);
        }

        group
    }
}