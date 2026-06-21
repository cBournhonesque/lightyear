use crate::SyncComponent;
use crate::plugin::{add_interpolation_systems, add_prepare_interpolation_systems};
use bevy_ecs::prelude::*;
use bevy_math::{
    Curve,
    curve::{Ease, EaseFunction, EasingCurve},
};
use bevy_platform::collections::HashMap;
use bevy_platform::collections::HashSet;
use bevy_replicon::bytes::Bytes;
use bevy_replicon::client::confirm_history::ConfirmHistory;
use bevy_replicon::prelude::{AppMarkerExt, RuleFns};
use bevy_replicon::shared::replication::deferred_entity::DeferredEntity;
use bevy_replicon::shared::replication::receive_markers::MarkerConfig;
use bevy_replicon::shared::replication::registry::ctx::{RemoveCtx, WriteCtx};
use bevy_utils::prelude::DebugName;
use lightyear_core::history_buffer::HistoryState;
use lightyear_core::prelude::{ConfirmedHistory, Interpolated, Tick};
use lightyear_replication::checkpoint::{ReplicationCheckpointMap, resolve_message_tick};
use lightyear_replication::registry::replication::ComponentRegistration;
use lightyear_replication::registry::{ComponentKind, ComponentRegistry, LerpFn};
use tracing::{error, trace};

fn lerp<C: Ease + Clone>(start: C, other: C, t: f32) -> C {
    let curve = EasingCurve::new(start, other, EaseFunction::Linear);
    curve.sample_unchecked(t)
}

#[derive(Debug, Clone)]
pub struct InterpolationMetadata {
    pub interpolation: Option<unsafe fn()>,
    pub custom_interpolation: bool,
}

#[derive(Resource, Debug, Default)]
pub struct InterpolationRegistry {
    pub(crate) interpolation_map: HashMap<ComponentKind, InterpolationMetadata>,
}

#[derive(Resource, Debug, Default)]
struct InterpolatedMarkerFnRegistry {
    kinds: HashSet<ComponentKind>,
}

impl InterpolationRegistry {
    pub fn set_linear_interpolation<C: Component + Clone + Ease>(&mut self) {
        self.set_interpolation(lerp::<C>);
    }

    pub fn set_interpolation<C: Component + Clone>(&mut self, interpolation_fn: LerpFn<C>) {
        let kind = ComponentKind::of::<C>();
        self.interpolation_map
            .entry(kind)
            .or_insert_with(|| InterpolationMetadata {
                interpolation: None,
                custom_interpolation: false,
            })
            .interpolation = Some(unsafe { core::mem::transmute(interpolation_fn) });
    }

    /// Returns True if the component `C` is interpolated
    pub fn interpolated<C: Component>(&self) -> bool {
        let kind = ComponentKind::of::<C>();
        self.interpolation_map.get(&kind).is_some()
    }

    pub(crate) fn has_interpolation_fn<C: Component>(&self) -> bool {
        let kind = ComponentKind::of::<C>();
        self.interpolation_map
            .get(&kind)
            .is_some_and(|metadata| metadata.interpolation.is_some())
    }

    pub fn interpolate<C: Component>(&self, start: C, end: C, t: f32) -> C {
        let kind = ComponentKind::of::<C>();
        let interpolation_metadata = self
            .interpolation_map
            .get(&kind)
            .expect("the component is not part of the protocol");
        let interpolation_fn: LerpFn<C> =
            unsafe { core::mem::transmute(interpolation_metadata.interpolation.unwrap()) };
        interpolation_fn(start, end, t)
    }

    /// Sample `history` at `interpolation_tick`.
    ///
    /// Returns `None` when no authoritative state exists at or before the
    /// interpolation tick. Otherwise returns the resolved authoritative state:
    /// either a removal, the latest present value, or an interpolated value
    /// between the bracketing present samples.
    ///
    /// If there is no next present sample, sampling returns the resolved start
    /// value instead of extrapolating.
    pub(crate) fn sample<C: Component + Clone>(
        &self,
        history: &ConfirmedHistory<C>,
        interpolation_tick: Tick,
        interpolation_overstep: f32,
    ) -> Option<HistoryState<C>> {
        let previous_index = (0..history.len())
            .take_while(|i| {
                history
                    .get_nth_tick(*i)
                    .is_some_and(|tick| tick <= interpolation_tick)
            })
            .last()?;

        let (start_tick, start_state) = history.get_nth_state(previous_index)?;
        let HistoryState::Updated(start) = start_state else {
            return Some(HistoryState::Removed);
        };

        let Some((end_tick, HistoryState::Updated(end))) =
            history.get_nth_state(previous_index + 1)
        else {
            return Some(HistoryState::Updated(start.clone()));
        };

        if !self.has_interpolation_fn::<C>() {
            return Some(HistoryState::Updated(start.clone()));
        }

        // Clamp rather than extrapolate beyond the newest confirmed value. This
        // makes late packets converge to the freshest server state instead of
        // overshooting when motion changes direction.
        let fraction = (((interpolation_tick - start_tick) as f32 + interpolation_overstep)
            / (end_tick - start_tick) as f32)
            .clamp(0.0, 1.0);
        trace!(
            target: "lightyear_debug::interpolation",
            kind = "confirmed_history_sample",
            component = ?DebugName::type_name::<C>(),
            interpolation_tick = interpolation_tick.0,
            start_tick = start_tick.0,
            end_tick = end_tick.0,
            interpolation_overstep,
            fraction,
            history_len = history.len(),
            "sampled confirmed history for interpolation"
        );
        Some(HistoryState::Updated(self.interpolate(
            start.clone(),
            end.clone(),
            fraction,
        )))
    }
}

fn register_interpolated_marker_fns<C: SyncComponent>(app: &mut bevy_app::App) {
    if !app
        .world()
        .contains_resource::<InterpolatedMarkerFnRegistry>()
    {
        app.world_mut()
            .insert_resource(InterpolatedMarkerFnRegistry::default());
    }
    let kind = ComponentKind::of::<C>();
    let already_registered = {
        let registry = app.world().resource::<InterpolatedMarkerFnRegistry>();
        registry.kinds.contains(&kind)
    };
    if already_registered {
        return;
    }
    app.register_marker_with::<Interpolated>(MarkerConfig {
        priority: 100,
        need_history: true,
    });
    app.set_marker_fns::<Interpolated, C>(write_history::<C>, remove_history::<C>);
    app.world_mut()
        .resource_mut::<InterpolatedMarkerFnRegistry>()
        .kinds
        .insert(kind);
}

/// When `Interpolated` is added after component `C` was already replicated onto the entity,
/// seed `ConfirmedHistory<C>` from the current value so interpolation has an anchor immediately.
///
/// Component updates for interpolated entities are normally captured by `write_history::<C>`, but
/// that only runs on future network updates. If `Interpolated` arrives after `C`, synthesize the
/// initial history entry from the existing component value and the entity's latest confirmed
/// Replicon tick.
pub(crate) fn insert_confirmed_history_on_interpolated<C: SyncComponent>(
    trigger: On<Add, Interpolated>,
    mut commands: Commands,
    checkpoints: Res<ReplicationCheckpointMap>,
    query: Query<(&C, &ConfirmHistory), Without<ConfirmedHistory<C>>>,
) {
    let Ok((component, confirm_history)) = query.get(trigger.entity) else {
        return;
    };

    let Some(tick) = checkpoints.get(confirm_history.last_tick()) else {
        debug_assert!(
            false,
            "missing authoritative checkpoint mapping while backfilling ConfirmedHistory"
        );
        return;
    };

    let mut history = ConfirmedHistory::<C>::default();
    history.insert_present(tick, component.clone());
    commands
        .entity(trigger.entity)
        .try_insert(history)
        .try_remove::<C>();
}

pub trait InterpolationRegistrationExt<C> {
    /// Register an interpolation function for this component using the provided [`LerpFn`]
    ///
    /// This does NOT mean that interpolation systems are added, it simply registers a function to
    /// interpolate between two values, that can be used for example in frame interpolation.
    fn register_interpolation_fn(self, interpolation_fn: LerpFn<C>) -> Self
    where
        C: SyncComponent;

    /// Register an interpolation function for this component using the [`Ease`] implementation
    ///
    /// This does NOT mean that interpolation systems are added, it simply registers a function to
    /// interpolate between two values, that can be used for example in frame interpolation.
    fn register_linear_interpolation(self) -> Self
    where
        C: SyncComponent + Ease;

    /// Add interpolation for this component using the provided [`LerpFn`]
    ///
    /// This will register interpolation systems to interpolate between two confirmed states.
    fn add_interpolation_with(self, interpolation_fn: LerpFn<C>) -> Self
    where
        C: SyncComponent;

    /// Enable interpolation systems for this component using the [`Ease`] implementation
    ///
    /// This will register interpolation systems to interpolate between two confirmed states.
    fn add_linear_interpolation(self) -> Self
    where
        C: SyncComponent + Ease;

    /// The remote updates will be stored in a [`ConfirmedHistory<C>`] component
    /// but the user has to define the interpolation logic themselves
    /// (`lightyear` won't perform any kind of interpolation)
    fn add_custom_interpolation(self) -> Self
    where
        C: SyncComponent;
}

impl<C> InterpolationRegistrationExt<C> for ComponentRegistration<'_, C> {
    fn register_interpolation_fn(self, interpolation_fn: LerpFn<C>) -> Self
    where
        C: SyncComponent,
    {
        register_interpolated_marker_fns::<C>(self.app);
        if !self
            .app
            .world()
            .contains_resource::<InterpolationRegistry>()
        {
            self.app
                .world_mut()
                .insert_resource(InterpolationRegistry::default());
        }
        let mut registry = self.app.world_mut().resource_mut::<InterpolationRegistry>();
        registry.set_interpolation::<C>(interpolation_fn);
        self
    }

    fn register_linear_interpolation(self) -> Self
    where
        C: SyncComponent + Ease,
    {
        self.register_interpolation_fn(lerp::<C>)
    }

    fn add_interpolation_with(mut self, interpolation_fn: LerpFn<C>) -> Self
    where
        C: SyncComponent,
    {
        self = self.register_interpolation_fn(interpolation_fn);
        add_prepare_interpolation_systems::<C>(self.app);
        add_interpolation_systems::<C>(self.app);

        let mut registry = self.app.world_mut().resource_mut::<ComponentRegistry>();
        registry
            .component_metadata_map
            .get_mut(&ComponentKind::of::<C>())
            .unwrap()
            .replication
            .as_mut()
            .unwrap()
            .set_interpolated(true);
        self
    }

    fn add_linear_interpolation(self) -> Self
    where
        C: SyncComponent + Ease,
    {
        self.add_interpolation_with(lerp::<C>)
    }

    fn add_custom_interpolation(self) -> Self
    where
        C: SyncComponent,
    {
        let kind = ComponentKind::of::<C>();
        register_interpolated_marker_fns::<C>(self.app);
        if !self
            .app
            .world()
            .contains_resource::<InterpolationRegistry>()
        {
            self.app
                .world_mut()
                .insert_resource(InterpolationRegistry::default());
        }
        let mut registry = self.app.world_mut().resource_mut::<InterpolationRegistry>();
        registry
            .interpolation_map
            .entry(kind)
            .and_modify(|r| r.custom_interpolation = true)
            .or_insert_with(|| InterpolationMetadata {
                interpolation: None,
                custom_interpolation: true,
            });
        add_prepare_interpolation_systems::<C>(self.app);

        let mut registry = self.app.world_mut().resource_mut::<ComponentRegistry>();
        registry
            .component_metadata_map
            .get_mut(&ComponentKind::of::<C>())
            .unwrap()
            .replication
            .as_mut()
            .unwrap()
            .set_interpolated(true);
        self
    }
}

/// Instead of writing into a component directly, it writes data into [`ConfirmedHistory<C>`].
fn write_history<C: SyncComponent>(
    ctx: &mut WriteCtx,
    rule_fns: &RuleFns<C>,
    entity: &mut DeferredEntity,
    message: &mut Bytes,
) -> bevy_ecs::error::Result<()> {
    let component: C = rule_fns.deserialize(ctx, message)?;
    // SAFETY: we only access resources, which don't alias with the DeferredEntity's component access.
    let checkpoints = {
        let world = unsafe { entity.world_mut() };
        let checkpoints =
            world.resource::<ReplicationCheckpointMap>() as *const ReplicationCheckpointMap;
        unsafe { &*checkpoints }
    };
    let Some(tick) = resolve_message_tick(checkpoints, ctx.message_tick) else {
        error!(
            message_tick = ?ctx.message_tick,
            "missing authoritative checkpoint mapping while writing interpolation history"
        );
        debug_assert!(
            false,
            "missing authoritative checkpoint mapping while writing interpolation history"
        );
        return Ok(());
    };
    if let Some(mut history) = entity.get_mut::<ConfirmedHistory<C>>() {
        history.insert_present(tick, component);
    } else {
        let mut history = ConfirmedHistory::<C>::default();
        history.insert_present(tick, component);
        entity.insert(history);
    }
    Ok(())
}

/// Records a component removal in `ConfirmedHistory<C>`.
///
/// The live component is removed later by interpolation systems once the interpolation timeline
/// reaches the server tick that produced this removal.
fn remove_history<C: SyncComponent>(ctx: &mut RemoveCtx, entity: &mut DeferredEntity) {
    // SAFETY: we only access resources, which don't alias with the DeferredEntity's component access.
    let checkpoints = {
        let world = unsafe { entity.world_mut() };
        let checkpoints =
            world.resource::<ReplicationCheckpointMap>() as *const ReplicationCheckpointMap;
        unsafe { &*checkpoints }
    };
    let Some(tick) = resolve_message_tick(checkpoints, ctx.message_tick) else {
        error!(
            message_tick = ?ctx.message_tick,
            "missing authoritative checkpoint mapping while recording interpolation removal"
        );
        debug_assert!(
            false,
            "missing authoritative checkpoint mapping while recording interpolation removal"
        );
        return;
    };
    if let Some(mut history) = entity.get_mut::<ConfirmedHistory<C>>() {
        history.insert_removed(tick);
    } else {
        let mut history = ConfirmedHistory::<C>::default();
        history.insert_removed(tick);
        entity.insert(history);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy_app::App;
    use bevy_ecs::component::Component;
    use bevy_replicon::prelude::RepliconTick;

    #[derive(Component, Clone, Debug, PartialEq)]
    struct TestComp(f32);

    fn lerp(start: TestComp, end: TestComp, t: f32) -> TestComp {
        TestComp(start.0 + (end.0 - start.0) * t)
    }

    fn registry() -> InterpolationRegistry {
        let mut registry = InterpolationRegistry::default();
        registry.set_interpolation::<TestComp>(lerp);
        registry
    }

    #[test]
    fn sample_clamps_to_newest_value_when_tick_is_past_end() {
        let mut history = ConfirmedHistory::<TestComp>::default();
        history.insert_present(Tick(10), TestComp(0.0));
        history.insert_present(Tick(20), TestComp(10.0));

        let registry = registry();
        assert_eq!(
            registry.sample(&history, Tick(30), 0.0),
            Some(HistoryState::Updated(TestComp(10.0)))
        );
        assert_eq!(
            registry.sample(&history, Tick(20), 0.5),
            Some(HistoryState::Updated(TestComp(10.0)))
        );
    }

    #[test]
    fn sample_returns_start_value_with_single_keyframe() {
        let mut history = ConfirmedHistory::<TestComp>::default();
        history.insert_present(Tick(10), TestComp(42.0));

        let registry = registry();
        assert_eq!(registry.sample(&history, Tick(5), 0.0), None);
        assert_eq!(
            registry.sample(&history, Tick(10), 0.0),
            Some(HistoryState::Updated(TestComp(42.0)))
        );
        assert_eq!(
            registry.sample(&history, Tick(50), 0.5),
            Some(HistoryState::Updated(TestComp(42.0)))
        );
    }

    #[test]
    fn inserts_history_when_interpolated_added_after_component_is_already_replicated() {
        let mut app = App::new();
        app.insert_resource(ReplicationCheckpointMap::default());
        app.add_observer(insert_confirmed_history_on_interpolated::<TestComp>);

        let replicon_tick = RepliconTick::new(11);
        app.world_mut()
            .resource_mut::<ReplicationCheckpointMap>()
            .record(replicon_tick, Tick(42));

        let entity = app
            .world_mut()
            .spawn((TestComp(2.0), ConfirmHistory::new(replicon_tick)))
            .id();
        app.update();
        app.world_mut().entity_mut(entity).insert(Interpolated);
        app.update();

        let history = app
            .world()
            .entity(entity)
            .get::<ConfirmedHistory<TestComp>>()
            .unwrap();
        assert_eq!(
            history
                .start_present()
                .map(|(tick, value)| (tick, value.clone())),
            Some((Tick(42), TestComp(2.0)))
        );
        assert!(
            !app.world().entity(entity).contains::<TestComp>(),
            "live interpolated component should be removed until the interpolation timeline reaches the history start tick"
        );
    }
}
