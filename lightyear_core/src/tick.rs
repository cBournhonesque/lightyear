use bevy::prelude::Resource;
use core::time::Duration;
use lightyear_utils::wrapping_id;

// Internal id that tracks the Tick value for the server and the client
wrapping_id!(Tick);


#[derive(Resource, Debug, Clone, Copy, PartialEq, Eq)]
pub struct TickDuration(pub Duration);