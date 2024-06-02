//! Provides a ArenaManager that wraps a slab allocator that can be used when
//! allocating a lot of small objects.
//!
//! This is best used when the allocations are needed in phases: a lot of small
//! objects need to be allocated, and then are suddenly not needed and can be
//! deallocated all at once.

use crate::prelude::MainSet;
use crate::shared::sets::{InternalReplicationSet, ServerMarker};
use bevy::prelude::*;
use blink_alloc::SyncBlinkAlloc;

#[derive(Default)]
pub struct ArenaPlugin;

impl ArenaPlugin {
    fn reset(mut arena: ResMut<ArenaManager>) {
        arena.allocator.reset();
    }
}

impl Plugin for ArenaPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ArenaManager>();

        app.add_systems(
            PostUpdate,
            Self::reset
                .in_set(MainSet::Send)
                .after(MainSet::SendPackets),
        );
    }
}

#[derive(Resource)]
pub struct ArenaManager {
    pub(crate) allocator: SyncBlinkAlloc,
}

impl ArenaManager {
    pub(crate) fn get(&self) -> &'static SyncBlinkAlloc {
        unsafe { std::mem::transmute(&self.allocator) }
    }
}

impl Default for ArenaManager {
    fn default() -> Self {
        Self {
            allocator: SyncBlinkAlloc::new(),
        }
    }
}
