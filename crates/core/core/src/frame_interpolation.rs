use bevy_ecs::prelude::*;
use bevy_reflect::Reflect;
use serde::{Deserialize, Serialize};

/// Marker component enabling frame interpolation on an entity.
///
/// Frame interpolation is type-erased: the entity only needs this marker, and
/// Lightyear selects the highest-priority matching interpolation rule for each
/// component or bundle on the entity's archetype.
///
/// # Examples
///
/// ```rust,ignore
/// use bevy_ecs::prelude::*;
/// use lightyear_core::prelude::FrameInterpolate;
///
/// fn spawn(mut commands: Commands) {
///     commands.spawn(FrameInterpolate);
/// }
/// ```
#[derive(Component, PartialEq, Serialize, Deserialize, Clone, Debug, Reflect)]
pub struct FrameInterpolate;

/// Component history used by frame interpolation for component `C`.
///
/// Lightyear inserts and updates this component for every matching rule that
/// owns frame interpolation history. Users can also write it directly when they
/// run custom rollback or interpolation systems.
///
/// # Examples
///
/// ```rust,ignore
/// use bevy_ecs::prelude::*;
/// use lightyear_core::prelude::FrameInterpolationHistory;
///
/// #[derive(Component, Clone, PartialEq)]
/// struct Position(f32);
///
/// fn seed(mut commands: Commands, entity: Entity) {
///     commands.entity(entity).insert(FrameInterpolationHistory::<Position> {
///         previous_value: Some(Position(0.0)),
///         current_value: Some(Position(1.0)),
///     });
/// }
/// ```
#[derive(Component, PartialEq, Debug)]
pub struct FrameInterpolationHistory<C: Component> {
    /// Value recorded at the previous fixed-update tick.
    pub previous_value: Option<C>,
    /// Value recorded at the current fixed-update tick.
    pub current_value: Option<C>,
}

impl<C: Component> Default for FrameInterpolationHistory<C> {
    fn default() -> Self {
        Self {
            previous_value: None,
            current_value: None,
        }
    }
}
