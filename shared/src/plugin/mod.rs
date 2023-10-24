use bevy::prelude::{App, Plugin};

pub use replication::ReplicationData;
pub use sets::ReplicationSet;

pub mod events;
mod replication;
mod sets;
pub mod systems;

pub struct SharedPlugin;

impl Plugin for SharedPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ReplicationData>();
    }
}
