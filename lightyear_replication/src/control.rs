use bevy::prelude::*;
use serde::{Deserialize, Serialize};


/// Marker component on the receiver side to indicate that the entity is under the
/// control of the local peer
#[derive(Component, Clone, Copy, PartialEq, Debug, Reflect, Serialize, Deserialize)]
#[reflect(Component)]
pub struct Controlled;


#[derive(Component, Clone, Copy, PartialEq, Debug, Reflect, Serialize, Deserialize)]
#[reflect(Component)]
pub struct ControlledBy;
