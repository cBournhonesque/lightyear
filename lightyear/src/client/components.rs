/*!
Defines components that are used for the client-side prediction and interpolation
*/
use std::fmt::Debug;

use bevy::prelude::{Component, Entity};

use crate::prelude::{MapEntities, Named};

/// Marks an entity that contains the server-updates that are received from the Server
/// (this entity is a copy of Predicted that is RTT ticks behind)
#[derive(Component)]
pub struct Confirmed {
    pub predicted: Option<Entity>,
    pub interpolated: Option<Entity>,
}

pub trait SyncComponent: Component + Clone + PartialEq + Named + for<'a> MapEntities<'a> {
    fn mode() -> ComponentSyncMode;
}

#[derive(Debug, Default, PartialEq)]
/// Defines how a predicted or interpolated component will be replicated from confirmed to predicted/interpolated
///
/// We use a single enum instead of 2 separate enums because we want to be able to use the same enum for both predicted and interpolated components
/// Otherwise it would be pretty tedious to have to set the values for both prediction and interpolation.
pub enum ComponentSyncMode {
    /// Sync the component from the confirmed to the interpolated/predicted entity with the most precision
    /// Predicted: we will check for rollback every tick
    /// Interpolated: we will run interpolation between the last 2 confirmed states
    Full,

    /// Simple sync: whenever the confirmed entity gets updated, we propagate the update to the interpolated/predicted entity
    /// Use this for components that don't get updated often or are not time-sensitive
    ///
    /// Predicted: that means the component's state will be ~1-RTT behind the predicted entity's timeline
    /// Interpolated: that means the component might not be rendered smoothly as it will only be updated after we receive a server update
    Simple,

    /// The component will be copied only-once from the confirmed to the interpolated/predicted entity, and then won't stay in sync
    /// Useful for components that you want to modify yourself on the predicted/interpolated entity
    Once,

    #[default]
    /// The component is not copied from the Confirmed entity to the interpolated/predicted entity
    None,
}
