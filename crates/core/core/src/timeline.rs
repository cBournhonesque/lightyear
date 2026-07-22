use crate::prelude::Tick;
use crate::tick::TickDuration;
use crate::time::{Overstep, TickDelta, TickInstant};
use bevy_app::{App, FixedFirst, Plugin};
use bevy_derive::{Deref, DerefMut};
use bevy_ecs::component::{Component, ComponentId, Mutable};
use bevy_ecs::entity::Entity;
use bevy_ecs::event::{EntityEvent, Event};
use bevy_ecs::prelude::{On, Resource};
use bevy_ecs::ptr::Ptr;
use bevy_ecs::query::With;
use bevy_ecs::schedule::SystemSet;
use bevy_ecs::system::{Query, ResMut};
use bevy_reflect::Reflect;
use bevy_time::{Fixed, Time};
use core::any::TypeId;
use core::ops::{Deref, DerefMut};
use core::time::Duration;
use lightyear_utils::collections::HashMap;
#[allow(unused_imports)]
use tracing::trace;

/// Shared ordering for systems that advance connection-local network timelines.
#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone, Copy)]
pub enum TimelineSystems {
    /// Drives the internal state of network timelines forward in `PreUpdate`.
    Advance,
}

/// Runtime identifier for a [`NetworkTimeline`] type.
///
/// This allows type-erased registries to identify a timeline without storing
/// an instance of it.
#[derive(Debug, Eq, Hash, Copy, Clone, PartialEq, Reflect)]
pub struct TimelineKind(TypeId);

impl TimelineKind {
    /// Returns the runtime identifier for timeline `T`.
    #[inline]
    pub fn of<T: NetworkTimeline>() -> Self {
        Self(TypeId::of::<T>())
    }
}

impl From<TypeId> for TimelineKind {
    fn from(type_id: TypeId) -> Self {
        Self(type_id)
    }
}

/// Type-erased access to a registered component timeline.
#[derive(Clone)]
pub struct TimelineMetadata {
    component_id: ComponentId,
    tick_fn: unsafe fn(Ptr<'_>) -> Tick,
}

impl TimelineMetadata {
    /// Component id for the registered timeline type.
    pub fn component_id(&self) -> ComponentId {
        self.component_id
    }

    /// Reads the current tick from a pointer to the registered timeline component.
    ///
    /// # Safety
    ///
    /// `timeline` must point to the component identified by
    /// [`component_id`](Self::component_id).
    pub unsafe fn tick(&self, timeline: Ptr<'_>) -> Tick {
        unsafe { (self.tick_fn)(timeline) }
    }
}

/// Registry of component timelines available for channel-delayed delivery.
#[derive(Resource, Default, Clone)]
pub struct TimelineRegistry {
    timelines: HashMap<TimelineKind, TimelineMetadata>,
}

impl TimelineRegistry {
    /// Registers type-erased access to timeline component `T`.
    pub fn register<T: NetworkTimeline>(&mut self, component_id: ComponentId) {
        self.timelines
            .entry(TimelineKind::of::<T>())
            .or_insert(TimelineMetadata {
                component_id,
                tick_fn: |ptr| {
                    // SAFETY: this callback is stored with the component id for `T`.
                    let timeline = unsafe { ptr.deref::<T>() };
                    timeline.tick()
                },
            });
    }

    /// Returns metadata for `kind`.
    pub fn get(&self, kind: &TimelineKind) -> Option<&TimelineMetadata> {
        self.timelines.get(kind)
    }

    /// Iterates over all registered timelines.
    pub fn iter(&self) -> impl Iterator<Item = (&TimelineKind, &TimelineMetadata)> {
        self.timelines.iter()
    }

    /// Iterates over registered timeline metadata.
    pub fn values(&self) -> impl Iterator<Item = &TimelineMetadata> {
        self.timelines.values()
    }
}

/// Timeline policy encoded by a channel registration.
///
/// [`LocalTimeline`] means immediate delivery. Component timelines register
/// themselves in [`TimelineRegistry`] and delay delivery until their current
/// tick reaches the payload's sender tick.
pub trait IntoMessageTimeline: Send + Sync + 'static {
    /// Runtime identity for delayed delivery, or `None` for immediate delivery.
    fn timeline_kind() -> Option<TimelineKind>;

    /// Registers any type-erased timeline access required by this policy.
    fn register(app: &mut App);
}

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
/// - the Tick is incremented every FixedUpdate (including during rollback)
/// - the overstep is set by the overstep of [`Time<Fixed>`]
#[derive(Resource, Deref, DerefMut, Default, Clone, Reflect)]
pub struct LocalTimeline {
    tick: Tick,
}

impl LocalTimeline {
    /// Get the current tick
    pub fn tick(&self) -> Tick {
        self.tick
    }

    /// Increment the LocalTimeline by `delta`
    pub fn apply_delta(&mut self, delta: i32) {
        self.tick = self.tick + delta;
    }
}

impl IntoMessageTimeline for LocalTimeline {
    fn timeline_kind() -> Option<TimelineKind> {
        None
    }

    fn register(app: &mut App) {
        app.init_resource::<TimelineRegistry>();
    }
}

impl<T: NetworkTimeline> IntoMessageTimeline for T {
    fn timeline_kind() -> Option<TimelineKind> {
        Some(TimelineKind::of::<T>())
    }

    fn register(app: &mut App) {
        app.init_resource::<TimelineRegistry>();
        let component_id = app.world_mut().register_component::<T>();
        app.world_mut()
            .resource_mut::<TimelineRegistry>()
            .register::<T>(component_id);
    }
}

/// Increment the local tick at each FixedUpdate
pub(crate) fn increment_local_tick(mut timeline: ResMut<LocalTimeline>) {
    timeline.tick += 1;
    trace!(
        target: "lightyear_debug::timeline",
        kind = "local_tick",
        sample_point = "FixedFirst",
        schedule = "FixedFirst",
        local_tick = timeline.tick.0,
        "local timeline tick advanced"
    );
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
    pub tick_delta: i32,
    marker: core::marker::PhantomData<T>,
}

impl<T: TimelineConfig> SyncEvent<T> {
    pub fn new(entity: Entity, tick_delta: i32) -> Self {
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
#[derive(Component, Debug, Clone, Copy)]
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
