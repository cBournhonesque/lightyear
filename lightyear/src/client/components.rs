/*!
Defines components that are used for the client-side prediction and interpolation
*/
use bevy::ecs::component::Mutable;
use bevy::prelude::{Component, Entity, ReflectComponent};
use bevy::reflect::Reflect;
use core::fmt::Debug;

use crate::prelude::{Message, Tick};

/// Marks an entity that directly applies the replication updates from the remote
///
/// In general, when an entity is replicated from the server to the client, multiple entities can be created on the client:
/// - an entity that simply contains the replicated components. It will have the marker component [`Confirmed`]
/// - an entity that is in the future compared to the confirmed entity, and does prediction with rollback. It will have the marker component [`Predicted`](crate::client::prediction::Predicted)
/// - an entity that is in the past compared to the confirmed entity and interpolates between multiple server updates. It will have the marker component [`Interpolated`](crate::client::interpolation::Interpolated)
#[derive(Component, Reflect, Default, Debug)]
#[reflect(Component)]
pub struct Confirmed {
    /// The corresponding Predicted entity
    pub predicted: Option<Entity>,
    /// The corresponding Interpolated entity
    pub interpolated: Option<Entity>,
    /// The tick that the confirmed entity is at.
    /// (this is latest server tick for which we applied updates to the entity)
    pub tick: Tick,
}

pub trait MutComponent: Component<Mutability = Mutable> {}

impl<T> MutComponent for T where T: Component<Mutability = Mutable> {}
pub trait SyncComponent: MutComponent + Clone + PartialEq + Message {}
impl<T> SyncComponent for T where T: MutComponent + Clone + PartialEq + Message {}

/// Function that will interpolate between two values
pub trait LerpFn<C> {
    fn lerp(start: &C, other: &C, t: f32) -> C;
}

/// Defines how to do interpolation/correction for the component
pub trait SyncMetadata<C> {
    type Interpolator: LerpFn<C> + 'static;
    type Corrector: LerpFn<C> + 'static;

    fn mode() -> ComponentSyncMode;
}

#[derive(Debug, Default, Clone, Copy, PartialEq)]
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
