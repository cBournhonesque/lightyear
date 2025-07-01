use bevy_ecs::component::Component;

pub mod input;
pub mod remote;
pub mod sync;

/// Marker component to identity the timeline that will update the bevy app.
///
/// [`Time<Virtual>`](bevy_time::Time<bevy_time::Virtual>) will be updated according to the driving timeline's relative_speed.
#[derive(Component, Default)]
pub struct DrivingTimeline<T> {
    pub marker: core::marker::PhantomData<T>,
}
