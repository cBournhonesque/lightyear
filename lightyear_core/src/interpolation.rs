use bevy_ecs::{component::Component, reflect::ReflectComponent};
use bevy_reflect::Reflect;

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
#[derive(Debug, Default, Reflect, Component, Clone, Copy)]
#[reflect(Component)]
pub struct Interpolated;
