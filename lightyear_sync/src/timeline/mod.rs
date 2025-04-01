use bevy::ecs::component::Mutable;
use bevy::prelude::Component;
use core::time::Duration;
use lightyear_core::tick::Tick;
use lightyear_core::time::{Overstep, TickDelta, TickInstant};

pub mod input;
pub mod remote;
#[cfg(feature = "interpolation")]
pub mod interpolation;
pub mod sync;

/// Marker component to identifty the local timeline, i.e. the timeline that corresponds to the bevy app.
///
/// Time<Virtual> will be updated according to the main timeline's relative_speed.
#[derive(Component, Default)]
pub struct LocalTimeline<T> {
    pub marker: core::marker::PhantomData<T>
}

/// The local timeline that matches Time<Virtual>
pub struct Local;

#[derive(Component, Default)]
pub struct Timeline<T: TimelineContext> {
    context: T,
    pub tick_duration: Duration,
    pub now: TickInstant,
    pub marker: core::marker::PhantomData<T>
}

impl<T: TimelineContext> From<T> for Timeline<T> {
    fn from(value: T) -> Self {
        Self {
            context: value,
            tick_duration: Default::default(),
            now: Default::default(),
            marker: Default::default(),
        }
    }
}

pub trait TimelineContext: Send + Sync + 'static {}

impl<T: Send + Sync + 'static> TimelineContext for T {}

pub trait NetworkTimeline: Component<Mutability=Mutable> {
    /// Estimate of the current time in the [`Timeline`]
    fn now(&self) -> TickInstant;

    fn tick_duration(&self) -> Duration;

    fn set_tick_duration(&mut self, duration: Duration);

    fn tick(&self) -> Tick;

    fn overstep(&self) -> Overstep;

    fn advance(&mut self, delta: Duration);
}


/// An extension trait for [`Time<Physics>`](Physics).
impl<T: TimelineContext> NetworkTimeline for Timeline<T> {

    /// Estimate of the current time in the [`Timeline`]
    fn now(&self) -> TickInstant {
        self.now
    }

    fn tick_duration(&self) -> Duration {
        self.tick_duration
    }

    fn set_tick_duration(&mut self, duration: Duration) {
        self.tick_duration = duration;
    }

    fn tick(&self) -> Tick {
        self.now().tick
    }

    fn overstep(&self) -> Overstep {
        self.now().overstep
    }

    fn advance(&mut self, delta: Duration) {
        self.now = self.now + TickDelta::from_duration(delta, self.tick_duration());
    }
}