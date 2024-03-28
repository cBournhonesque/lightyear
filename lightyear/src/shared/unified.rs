//! Resources related to running in unified mode (client and server in the same app).

use bevy::app::App;
use bevy::prelude::{Res, Resource};

/// Counter to check if both client and server are running in the same app.
#[derive(Resource, Default)]
pub struct UnifiedManager(bool);

impl UnifiedManager {
    pub fn is_unified(&self) -> bool {
        self.0
    }

    pub(crate) fn add_or_increment(app: &mut App) {
        if let Some(mut counter) = app.world.get_resource_mut::<UnifiedManager>() {
            counter.0 = true;
        } else {
            app.init_resource::<UnifiedManager>();
        }
    }

    /// Run condition that returns true if we are running in unified mode
    pub fn is_unified_condition(counter: Res<UnifiedManager>) -> bool {
        counter.0
    }
}
