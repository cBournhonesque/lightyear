use bevy_ecs::{component::Component, reflect::ReflectComponent};
use bevy_reflect::Reflect;

/// Marker component to identify this link as a LinkOf
///
/// This is equivalent to LinkOf + Connected.
#[derive(Component, Clone, Copy, PartialEq, Eq, Debug, Reflect)]
#[reflect(Component, PartialEq, Debug, Clone)]
pub struct ClientOf;
