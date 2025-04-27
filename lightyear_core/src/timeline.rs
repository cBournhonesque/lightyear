use crate::prelude::Tick;
use crate::tick::TickDuration;
use crate::time::{Overstep, TickDelta, TickInstant};
use bevy::app::{App, FixedFirst, Plugin, RunFixedMainLoop, RunFixedMainLoopSystem};
use bevy::ecs::component::{HookContext, Mutable};
use bevy::ecs::world::DeferredWorld;
use bevy::prelude::{Component, Deref, DerefMut, Event, Fixed, IntoScheduleConfigs, Query, Reflect, Res, ResMut, Resource, Time, Trigger};
use bevy::reflect::GetTypeRegistration;
use core::ops::{Deref, DerefMut};
use core::time::Duration;
use parking_lot::RwLock;

#[derive(Default, Debug, Clone, Reflect)]
pub struct Timeline<T: TimelineContext> {
    pub context: T,
    pub now: TickInstant,
    #[reflect(ignore)]
    pub marker: core::marker::PhantomData<T>
}



impl<T: TimelineContext> From<T> for Timeline<T> {
    fn from(value: T) -> Self {
        Self {
            context: value,
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

    fn tick(&self) -> Tick;

    fn overstep(&self) -> Overstep;

    fn apply_delta(&mut self, delta: TickDelta);

    fn apply_duration(&mut self, duration: Duration, tick_duration: Duration) {
        self.apply_delta(TickDelta::from_duration(duration, tick_duration));
    }
}


// /// An extension trait for [`Time<Physics>`](Physics).
// impl<T: TimelineContext> NetworkTimeline for Timeline<T> {
//
//     /// Estimate of the current time in the [`Timeline`]
//     fn now(&self) -> TickInstant {
//         self.now
//     }
//
//     fn tick_duration(&self) -> Duration {
//         self.tick_duration
//     }
//
//     fn set_tick_duration(&mut self, duration: Duration) {
//         self.tick_duration = duration;
//     }
//
//     fn tick(&self) -> Tick {
//         self.now().tick
//     }
//
//     fn overstep(&self) -> Overstep {
//         self.now().overstep
//     }
//
//     fn apply_delta(&mut self, delta: TickDelta) {
//         self.now = self.now + delta;
//     }
// }

impl<C: TimelineContext, T: Component<Mutability=Mutable> + DerefMut<Target=Timeline<C>>> NetworkTimeline for T {

    /// Estimate of the current time in the [`Timeline`]
    fn now(&self) -> TickInstant {
        self.now
    }

    fn tick(&self) -> Tick {
        self.now().tick
    }

    fn overstep(&self) -> Overstep {
        self.now().overstep
    }

    fn apply_delta(&mut self, delta: TickDelta) {
        self.now = self.now + delta;
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


/// Resource that will track whether we should do rollback or not
/// (We have this as a resource because if any predicted entity needs to be rolled-back; we should roll back all predicted entities)
#[derive(Debug, Default, Reflect)]
pub enum RollbackState {
    /// We are not in a rollback state
    #[default]
    Default,
    /// We should do a rollback starting from the current_tick
    ShouldRollback {
        /// Current tick of the rollback process
        ///
        /// (note: we will start the rollback from the next tick after we notice the mismatch)
        current_tick: Tick,
    },
}

impl Local {}


/// The local timeline that matches Time<Virtual>
/// - the Tick is incremented every FixedUpdate
/// - the overstep is set by the overstep of Time<Fixed>
#[derive(Default, Clone, Reflect)]
pub struct Local;

#[derive(Component, Deref, DerefMut, Default, Clone, Reflect)]
pub struct LocalTimeline(Timeline<Local>);


/// Increment the local tick at each FixedUpdate
pub(crate) fn increment_local_tick(
    mut query: Query<&mut LocalTimeline>,
) {
    query.iter_mut().for_each(|mut t| {
        t.apply_delta(TickDelta::from_i16(1));
        // trace!("Timeline::advance: now: {:?}, duration: {:?}", t.now(), duration);
    })
}



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

impl<T: NetworkTimeline> Plugin for NetworkTimelinePlugin<T> {
    fn build(&self, app: &mut App) {
    }
}


/// Event that can be triggered to update the tick duration.
///
/// If the trigger is global, it will update:
/// - Time<Fixed>
/// - the various Timelines
///
/// The event can also be triggered for a specific target to update only the components of that target.
#[derive(Event)]
pub struct SetTickDuration(pub Duration);

pub struct TimelinePlugin {
    pub(crate) tick_duration: Duration
}

impl TimelinePlugin {
    fn update_tick_duration(trigger: Trigger<SetTickDuration>, mut time: ResMut<Time<Fixed>>) {
        time.set_timestep(trigger.0);
    }
}

impl Plugin for TimelinePlugin {
    fn build(&self, app: &mut App) {
        // TODO: this should be in InputPlugin
        app.register_type::<RollbackState>();

        app.insert_resource(TickDuration(self.tick_duration));
        app.world_mut().resource_mut::<Time<Fixed>>().set_timestep(self.tick_duration);
        app.add_observer(Self::update_tick_duration);

        app.add_plugins(NetworkTimelinePlugin::<LocalTimeline>::default());
        app.add_systems(FixedFirst, increment_local_tick);
    }

    fn finish(&self, app: &mut App) {
        // After timelines and PingManager are created, trigger a TickDuration event
        app.world_mut().trigger(SetTickDuration(self.tick_duration));
    }
}