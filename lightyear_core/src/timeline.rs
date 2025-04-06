use crate::prelude::Tick;
use crate::time::{Overstep, SetTickDuration, TickDelta, TickInstant};
use bevy::app::{App, FixedFirst, Plugin, RunFixedMainLoop, RunFixedMainLoopSystem};
use bevy::ecs::component::Mutable;
use bevy::prelude::{Component, Fixed, IntoScheduleConfigs, Query, Res, Time, Trigger};
use core::ops::{Deref, DerefMut};
use core::time::Duration;

#[derive(Component, Default)]
pub struct Timeline<T: TimelineContext> {
    pub context: T,
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

// TODO: should we get rid of this trait and just use the Timeline<T> struct?
//  maybe a trait gives us more options in the future
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

impl<T: TimelineContext> Deref for Timeline<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.context
    }
}

impl<T: TimelineContext> DerefMut for  Timeline<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.context
    }
}


/// The local timeline that matches Time<Virtual>
/// - the Tick is incremented every FixedUpdate
/// - the overstep is set by the overstep of Time<Fixed>
#[derive(Default)]
pub struct Local;

pub type LocalTimeline = Timeline<Local>;


/// Increment the local tick at each FixedUpdate
pub(crate) fn increment_local_tick(
    mut query: Query<&mut Timeline<Local>>,
) {
    query.iter_mut().for_each(|mut t| {
        let duration = t.tick_duration();
        t.advance(duration);
    })
}

/// Update the overstep using the Time<Fixed> overstep
pub(crate) fn set_local_overstep(
    fixed_time: Res<Time<Fixed>>,
    mut query: Query<&mut Timeline<Local>>,
) {
    let overstep = fixed_time.overstep();
    query.iter_mut().for_each(|mut t| {
        t.advance(overstep);
    })
}


pub struct TimelinePlugin;


pub struct NetworkTimelinePlugin<T> {
    pub(crate) _marker: core::marker::PhantomData<T>,
}


impl<T> Default for NetworkTimelinePlugin<T> {
    fn default() -> Self {
        Self {
            _marker: core::marker::PhantomData,
        }
    }
}

impl<T: TimelineContext> NetworkTimelinePlugin<T> where Timeline<T>: NetworkTimeline  {
    pub(crate) fn update_tick_duration(
        trigger: Trigger<SetTickDuration>,
        mut query: Query<&mut Timeline<T>>,
    ) {
        if let Ok(mut t) = query.get_mut(trigger.target()) {
            t.set_tick_duration(trigger.0);
        }
    }
}

impl<T: TimelineContext> Plugin for NetworkTimelinePlugin<T> where Timeline<T>: NetworkTimeline {
    fn build(&self, app: &mut App) {
        app.add_observer(Self::update_tick_duration);
    }
}

impl Plugin for TimelinePlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(NetworkTimelinePlugin::<Local>::default());
        app.add_systems(FixedFirst, increment_local_tick);
        app.add_systems(RunFixedMainLoop, set_local_overstep.in_set(RunFixedMainLoopSystem::AfterFixedMainLoop));
    }
}