use core::time::Duration;
use lightyear_utils::wrapping_id;

use bevy_derive::{Deref, DerefMut};
use bevy_ecs::resource::Resource;
use bevy_reflect::Reflect;

// Internal id that tracks the Tick value for the server and the client
wrapping_id!(Tick);

/// Resource that contains the global TickDuration
#[derive(Resource, Debug, Clone, Copy, PartialEq, Eq, Reflect, Deref, DerefMut)]
pub struct TickDuration(pub Duration);
