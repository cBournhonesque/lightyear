use crate::prelude::Tick;
use crate::tick::TickDuration;
use crate::time::{Overstep, TickDelta, TickInstant};
use bevy_app::{App, FixedFirst, Plugin};
use bevy_derive::{Deref, DerefMut};
use bevy_ecs::component::{Component, Mutable};
use bevy_ecs::entity::Entity;
use bevy_ecs::event::{EntityEvent, Event};
use bevy_ecs::prelude::{On, Resource};
use bevy_ecs::query::With;
use bevy_ecs::system::{Query, ResMut};
use bevy_reflect::Reflect;
use bevy_time::{Fixed, Time};
use core::ops::{Deref, DerefMut};
use core::time::Duration;

/// A timeline defines an independent progression of time.
///
/// A given entity can be associated with multiple timelines.
/// Each Timeline is associated with a [`TimelineConfig`] component that is used to
/// configure the timeline.
#[derive(Default, Debug, Clone, Reflect)]
pub struct Timeline<T: TimelineConfig> {
    pub context: T::Context,
    pub now: TickInstant,
    #[reflect(ignore)]
    pub marker: core::marker::PhantomData<T>,
}

/// Configuration for a [`Timeline`].
///
/// The user should only modify the configuration.
pub trait TimelineConfig: Component + Send + Sync + Sized + 'static {
    /// Contextual data associated with this timeline configuration; used by the timeline's internals
    type Context;

    type Timeline: NetworkTimeline + Default;
}

// TODO: should we get rid of this trait and just use the Timeline<T> struct?
//  maybe a trait gives us more options in the future
pub trait NetworkTimeline: Component<Mutability = Mutable> {
    type Config: TimelineConfig;
    const PAUSED_DURING_ROLLBACK: bool = true;

    /// Estimate of the current time in the [`Timeline`]
    fn now(&self) -> TickInstant;

    fn tick(&self) -> Tick;

    fn overstep(&self) -> Overstep;

    fn set_now(&mut self, now: TickInstant);

    fn apply_delta(&mut self, delta: TickDelta);

    fn apply_duration(&mut self, duration: Duration, tick_duration: Duration) {
        self.apply_delta(TickDelta::from_duration(duration, tick_duration));
    }
}

impl<C: TimelineConfig, T: Component<Mutability = Mutable> + DerefMut<Target = Timeline<C>>>
    NetworkTimeline for T
{
    type Config = C;

    /// Estimate of the current time in the [`Timeline`]
    fn now(&self) -> TickInstant {
        self.now
    }

    fn tick(&self) -> Tick {
        self.now().tick()
    }

    fn overstep(&self) -> Overstep {
        self.now().overstep()
    }

    fn set_now(&mut self, now: TickInstant) {
        self.now = now;
    }

    fn apply_delta(&mut self, delta: TickDelta) {
        self.now = self.now + delta;
    }
}

impl<T: TimelineConfig> Deref for Timeline<T> {
    type Target = T::Context;

    fn deref(&self) -> &Self::Target {
        &self.context
    }
}

impl<T: TimelineConfig> DerefMut for Timeline<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.context
    }
}

/// The local timeline that matches [`Time<Virtual>`]
/// - the Tick is incremented every FixedUpdate
/// - the overstep is set by the overstep of [`Time<Fixed>`]
#[derive(Resource, Deref, DerefMut, Default, Clone, Reflect)]
pub struct LocalTimeline {
    tick: Tick,
}

impl LocalTimeline {
    pub fn tick(&self) -> Tick {
        self.tick
    }
    pub fn apply_delta(&mut self, delta: i16) {
        self.tick = self.tick + delta;
    }
}

/// Increment the local tick at each FixedUpdate
pub(crate) fn increment_local_tick(mut local: ResMut<LocalTimeline>) {
    local.tick += 1;
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
    fn build(&self, _: &mut App) {}
}

/// Event that can be triggered to update the tick duration.
///
/// If the trigger is global, it will update:
/// - [`Time<Fixed>`]
/// - the various Timelines
///
/// The event can also be triggered for a specific target to update only the components of that target.
#[derive(Event)]
pub struct SetTickDuration(pub Duration);

pub struct TimelinePlugin {
    pub(crate) tick_duration: Duration,
}

impl TimelinePlugin {
    fn update_tick_duration(trigger: On<SetTickDuration>, mut time: ResMut<Time<Fixed>>) {
        time.set_timestep(trigger.0);
    }
}

impl Plugin for TimelinePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<LocalTimeline>();
        app.insert_resource(TickDuration(self.tick_duration));
        app.world_mut()
            .resource_mut::<Time<Fixed>>()
            .set_timestep(self.tick_duration);
        app.add_observer(Self::update_tick_duration);

        app.add_systems(FixedFirst, increment_local_tick);
    }

    fn finish(&self, app: &mut App) {
        // After timelines and PingManager are created, trigger a TickDuration event
        app.world_mut().trigger(SetTickDuration(self.tick_duration));
    }
}

#[derive(EntityEvent, Debug)]
pub struct SyncEvent<T: TimelineConfig> {
    /// Entity holding a [`Timeline`]
    pub entity: Entity,
    // NOTE: it's inconvenient to re-sync the Timeline from a TickInstant to another TickInstant,
    //  so instead we will apply a delta number of ticks with no overstep (so that it's easy
    //  to update the LocalTimeline
    /// Delta in number of ticks to apply to the timeline
    pub tick_delta: i16,
    marker: core::marker::PhantomData<T>,
}

impl<T: TimelineConfig> SyncEvent<T> {
    pub fn new(entity: Entity, tick_delta: i16) -> Self {
        SyncEvent {
            entity,
            tick_delta,
            marker: core::marker::PhantomData,
        }
    }
}

impl<T: TimelineConfig> Clone for SyncEvent<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: TimelineConfig> Copy for SyncEvent<T> {}

/// Marker component inserted on the Link if we are currently in rollback
///
/// This is in `lightyear_core` to avoid circular dependencies. Many other plugins behave differently during rollback
#[derive(Component)]
pub enum Rollback {
    /// The rollback is initiated because we have received new Confirmed state from the server
    /// that doesn't match our prediction history.
    FromState,
    /// The rollback is initiated because we have received new Inputs for remote clients
    ///
    /// We should still check if there are state mismatches
    FromInputs,
}

/// Run condition to check if we are in rollback
pub fn is_in_rollback(client: Query<(), With<Rollback>>) -> bool {
    client.single().is_ok()
}
