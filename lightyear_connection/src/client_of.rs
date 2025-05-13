use bevy::ecs::relationship::RelationshipSourceCollection;
use bevy::prelude::*;

/// Marker component to identify this link as a LinkOf
///
/// This is equivalent to LinkOf + Connected.
#[derive(Component, Clone, Copy, PartialEq, Eq, Debug, Reflect)]
#[reflect(Component, PartialEq, Debug, Clone)]
pub struct ClientOf;
