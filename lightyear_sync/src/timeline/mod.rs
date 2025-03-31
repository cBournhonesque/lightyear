use bevy::ecs::component::Mutable;
use bevy::prelude::Component;
use lightyear_core::tick::Tick;
use lightyear_core::time::{Overstep, TickInstant};
use std::time::Duration;

pub mod prediction;
pub mod remote;
pub mod interpolation;
pub mod sync;


/// Marker component to identifty the main timeline.
///
/// Time<Virtual> will be updated according to the main timeline's relative_speed.
#[derive(Component)]
pub struct Main<T: Timeline> {
    pub marker: core::marker::PhantomData<T>
}

/// An extension trait for [`Time<Physics>`](Physics).
pub trait Timeline: Component<Mutability=Mutable> {

    /// Estimate of the current time in the [`Timeline`]
    fn now(&self) -> TickInstant;

    fn tick_duration(&self) -> Duration;

    fn tick(&self) -> Tick {
        self.now().tick
    }

    fn overstep(&self) -> Overstep {
        self.now().overstep
    }

    fn advance(&mut self, delta: Duration);
}

pub struct Fixed;

pub struct Update;