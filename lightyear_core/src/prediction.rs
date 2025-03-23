use bevy::prelude::{Reflect, ReflectResource, Resource};

use crate::tick::Tick;
use core::ops::{Deref, DerefMut};
use parking_lot::RwLock;

/// Resource that indicates whether we are in a rollback state or not
#[derive(Default, Resource, Reflect)]
#[reflect(Resource)]
pub struct Rollback {
    // have to reflect(ignore) this field because of RwLock unfortunately
    #[reflect(ignore)]
    /// We use a RwLock because we want to be able to update this value from multiple systems
    /// in parallel.
    pub state: RwLock<RollbackState>,
    // pub rollback_groups: EntityHashMap<ReplicationGroupId, RollbackState>,
}

/// Resource that will track whether we should do rollback or not
/// (We have this as a resource because if any predicted entity needs to be rolled-back; we should roll back all predicted entities)
#[derive(Debug, Default, Reflect)]
pub enum RollbackState {
    /// We are not in a rollback state
    #[default]
    Default,
    /// We should do a rollback starting from the current_tick
    ShouldRollback {
        /// Current tick of the rollback process
        ///
        /// (note: we will start the rollback from the next tick after we notice the mismatch)
        current_tick: Tick,
    },
}

impl Rollback {
    pub(crate) fn new(state: RollbackState) -> Self {
        Self {
            state: RwLock::new(state),
        }
    }

    /// Returns true if we are currently in a rollback state
    pub fn is_rollback(&self) -> bool {
        match *self.state.read().deref() {
            RollbackState::ShouldRollback { .. } => true,
            RollbackState::Default => false,
        }
    }

    /// Get the current rollback tick
    pub fn get_rollback_tick(&self) -> Option<Tick> {
        match *self.state.read().deref() {
            RollbackState::ShouldRollback { current_tick } => Some(current_tick),
            RollbackState::Default => None,
        }
    }

    /// Increment the rollback tick
    pub(crate) fn increment_rollback_tick(&self) {
        if let RollbackState::ShouldRollback {
            ref mut current_tick,
        } = *self.state.write().deref_mut()
        {
            *current_tick += 1;
        }
    }

    /// Set the rollback state back to non-rollback
    pub(crate) fn set_non_rollback(&self) {
        *self.state.write().deref_mut() = RollbackState::Default;
    }

    /// Set the rollback state to `ShouldRollback` with the given tick
    pub(crate) fn set_rollback_tick(&self, tick: Tick) {
        *self.state.write().deref_mut() = RollbackState::ShouldRollback { current_tick: tick };
    }
}