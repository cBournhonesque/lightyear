#![no_std]

extern crate alloc;
#[cfg(feature = "std")]
extern crate std;

#[cfg(feature = "server")]
mod server;

#[cfg(feature = "client")]
mod client;
pub mod send;
mod registry;
mod metadata;
pub mod authority;
mod control;

use bevy_app::PluginGroupBuilder;
use bevy_app::prelude::PluginGroup;

pub struct LightyearRepliconBackend;

impl PluginGroup for LightyearRepliconBackend {
    fn build(self) -> PluginGroupBuilder {
        let mut group = PluginGroupBuilder::start::<Self>();

        #[cfg(feature = "server")]
        {
            group = group.add(server::RepliconServerPlugin);
        }

        #[cfg(feature = "client")]
        {
            group = group.add(client::RepliconClientPlugin);
        }

        group
    }
}