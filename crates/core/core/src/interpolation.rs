use bevy_ecs::{component::Component, reflect::ReflectComponent};
use bevy_reflect::Reflect;
use serde::{Deserialize, Serialize};

use crate::tick::Tick;

/// Component added to client-side entities that are visually interpolated.
///
/// Interpolation is used to smooth the visual representation of entities received from the server.
/// Instead of snapping to new positions/states upon receiving a server update, the entity's
/// components are smoothly transitioned from their previous state to the new state over time.
///
/// This component links the interpolated entity to its server-confirmed counterpart.
/// The `InterpolationPlugin` uses this to:
/// - Store the component history of the confirmed entity.
/// - Apply interpolated values to the components of this entity based on the `InterpolationTimeline`.
// NOTE: we create Interpolated here because it is used by multiple crates (interpolation, replication)
#[derive(Debug, Clone, Copy, Default, Reflect, Serialize, Deserialize, Component)]
#[reflect(Component)]
pub struct Interpolated;

/// Temporarily disables an entity whose [`Interpolated`] marker was received
/// through replication until the interpolation timeline reaches the marker's
/// authoritative server tick.
///
/// [`lightyear_interpolation`](https://docs.rs/lightyear_interpolation) registers
/// this as a Bevy disabling component. It is inserted by an observer after the
/// other [`Interpolated`] add observers have run. Client-local [`Interpolated`]
/// insertions do not receive this component. It is removed in the same structural
/// change that materializes the entity at the interpolation timeline.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Reflect, Component)]
#[reflect(Component)]
pub struct InterpolationPending {
    /// Authoritative server tick when the replicated interpolation marker was received.
    pub spawn_tick: Tick,
}
