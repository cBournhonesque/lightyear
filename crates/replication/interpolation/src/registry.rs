use crate::SyncComponent;
use crate::plugin::{
    add_interpolation_systems, add_prepare_interpolation_diff_systems,
    add_prepare_interpolation_systems,
};
use bevy_app::App;
use bevy_ecs::prelude::*;
use bevy_math::{
    Curve,
    curve::{Ease, EaseFunction, EasingCurve},
};
use bevy_platform::collections::HashMap;
use bevy_platform::collections::HashSet;
use bevy_replicon::bytes::Bytes;
use bevy_replicon::client::confirm_history::ConfirmHistory;
use bevy_replicon::postcard_utils;
use bevy_replicon::prelude::{AppMarkerExt, RuleFns};
use bevy_replicon::shared::replication::deferred_entity::DeferredEntity;
use bevy_replicon::shared::replication::diff::{
    ComponentDelta, DiffBuffer, Diffable as RepliconDiffable,
};
use bevy_replicon::shared::replication::receive_markers::MarkerConfig;
use bevy_replicon::shared::replication::registry::ctx::{RemoveCtx, WriteCtx};
use bevy_replicon::shared::replication::storage::{EntityStorageCtx, ReplicationStorage};
use bevy_utils::prelude::DebugName;
use lightyear_core::history_buffer::HistoryState;
use lightyear_core::prelude::{ConfirmedHistory, Interpolated, Tick};
use lightyear_replication::checkpoint::{ReplicationCheckpointMap, resolve_message_tick};
use lightyear_replication::diff_history::ConfirmedHistoryPatchReceiver;
use lightyear_replication::registry::replication::{ComponentRegistration, ComponentRegistrator};
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

fn register_interpolated_diff_marker_fns<C: SyncComponent + RepliconDiffable>(
    app: &mut bevy_app::App,
) {
    if !app
        .world()
        .contains_resource::<InterpolatedMarkerFnRegistry>()
    {
        app.world_mut()
            .insert_resource(InterpolatedMarkerFnRegistry::default());
    }
    let kind = ComponentKind::of::<C>();
    app.register_marker_with::<Interpolated>(MarkerConfig {
        priority: 100,
        need_history: true,
    });
    app.set_marker_fns::<Interpolated, C>(write_history_diff::<C>, remove_history_diff::<C>);
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

pub(crate) fn insert_confirmed_history_on_interpolated_diff<C: SyncComponent + RepliconDiffable>(
    trigger: On<Add, Interpolated>,
    mut commands: Commands,
    checkpoints: Res<ReplicationCheckpointMap>,
    query: Query<(&C, &ConfirmHistory, Option<&ConfirmedHistory<C>>)>,
) {
    let Ok((component, confirm_history, history)) = query.get(trigger.entity) else {
        return;
    };

    let Some(tick) = checkpoints.get(confirm_history.last_tick()) else {
        debug_assert!(
            false,
            "missing authoritative checkpoint mapping while backfilling diff ConfirmedHistory"
        );
        return;
    };

    let entity = trigger.entity;
    let component = component.clone();
    let insert_history = history.is_none();
    commands.queue(move |world: &mut World| {
        let (cursor, has_receiver) = world
            .get_resource::<ReplicationStorage>()
            .map(|storage| {
                (
                    storage
                        .get::<DiffBuffer<C>>(entity)
                        .and_then(DiffBuffer::<C>::last_applied),
                    storage
                        .get::<ConfirmedHistoryPatchReceiver<C>>(entity)
                        .is_some(),
                )
            })
            .unwrap_or_default();

        if !insert_history && has_receiver {
            return;
        }

        {
            let Ok(mut entity_mut) = world.get_entity_mut(entity) else {
                return;
            };
            if insert_history && !entity_mut.contains::<ConfirmedHistory<C>>() {
                let mut history = ConfirmedHistory::<C>::default();
                history.insert_present(tick, component);
                entity_mut.insert(history);
            }
            entity_mut.remove::<C>();
        }

        if !has_receiver
            && let Some(cursor) = cursor
            && let Some(mut storage) = world.get_resource_mut::<ReplicationStorage>()
            && storage
                .get::<ConfirmedHistoryPatchReceiver<C>>(entity)
                .is_none()
        {
            let mut receiver = ConfirmedHistoryPatchReceiver::<C>::default();
            receiver.record_cursor(tick, Some(cursor));
            storage.insert(entity, receiver);
        }
    });
}

pub trait InterpolationRegistrationExt<'a, C>: ComponentRegistrator<'a, C> {
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

    /// Like [`Self::add_interpolation_with`], but for components replicated with
    /// Replicon's diff-based mode.
    fn add_interpolation_diff_with(self, interpolation_fn: LerpFn<C>) -> Self
    where
        C: SyncComponent + RepliconDiffable;

    /// Enable interpolation systems for this component using the [`Ease`] implementation
    ///
    /// This will register interpolation systems to interpolate between two confirmed states.
    fn add_linear_interpolation(self) -> Self
    where
        C: SyncComponent + Ease;

    /// Like [`Self::add_linear_interpolation`], but for components replicated
    /// with Replicon's diff-based mode.
    fn add_linear_interpolation_diff(self) -> Self
    where
        C: SyncComponent + RepliconDiffable + Ease;

    /// The remote updates will be stored in a [`ConfirmedHistory<C>`] component
    /// but the user has to define the interpolation logic themselves
    /// (`lightyear` won't perform any kind of interpolation)
    fn add_custom_interpolation(self) -> Self
    where
        C: SyncComponent;

    /// Like [`Self::add_custom_interpolation`], but for components replicated
    /// with Replicon's diff-based mode.
    fn add_custom_interpolation_diff(self) -> Self
    where
        C: SyncComponent + RepliconDiffable;
}

impl<'a, C, R> InterpolationRegistrationExt<'a, C> for R
where
    R: ComponentRegistrator<'a, C>,
{
    fn register_interpolation_fn(self, interpolation_fn: LerpFn<C>) -> Self
    where
        C: SyncComponent,
    {
        Self::from_component_registration(register_interpolation_fn_impl(
            self.into_component_registration(),
            interpolation_fn,
        ))
    }

    fn register_linear_interpolation(self) -> Self
    where
        C: SyncComponent + Ease,
    {
        self.register_interpolation_fn(lerp::<C>)
    }

    fn add_interpolation_with(self, interpolation_fn: LerpFn<C>) -> Self
    where
        C: SyncComponent,
    {
        Self::from_component_registration(add_interpolation_with_impl(
            self.into_component_registration(),
            interpolation_fn,
        ))
    }

    fn add_interpolation_diff_with(self, interpolation_fn: LerpFn<C>) -> Self
    where
        C: SyncComponent + RepliconDiffable,
    {
        Self::from_component_registration(add_interpolation_diff_with_impl(
            self.into_component_registration(),
            interpolation_fn,
        ))
    }

    fn add_linear_interpolation(self) -> Self
    where
        C: SyncComponent + Ease,
    {
        self.add_interpolation_with(lerp::<C>)
    }

    fn add_linear_interpolation_diff(self) -> Self
    where
        C: SyncComponent + RepliconDiffable + Ease,
    {
        self.add_interpolation_diff_with(lerp::<C>)
    }

    fn add_custom_interpolation(self) -> Self
    where
        C: SyncComponent,
    {
        Self::from_component_registration(add_custom_interpolation_impl(
            self.into_component_registration(),
        ))
    }

    fn add_custom_interpolation_diff(self) -> Self
    where
        C: SyncComponent + RepliconDiffable,
    {
        Self::from_component_registration(add_custom_interpolation_diff_impl(
            self.into_component_registration(),
        ))
    }
}

fn ensure_interpolation_registry(app: &mut App) {
    if !app.world().contains_resource::<InterpolationRegistry>() {
        app.world_mut()
            .insert_resource(InterpolationRegistry::default());
    }
}

fn mark_interpolated<C: SyncComponent>(app: &mut App) {
    let mut registry = app.world_mut().resource_mut::<ComponentRegistry>();
    registry
        .component_metadata_map
        .get_mut(&ComponentKind::of::<C>())
        .unwrap()
        .replication
        .as_mut()
        .unwrap()
        .set_interpolated(true);
}

fn register_interpolation_fn_impl<'a, C>(
    registration: ComponentRegistration<'a, C>,
    interpolation_fn: LerpFn<C>,
) -> ComponentRegistration<'a, C>
where
    C: SyncComponent,
{
    register_interpolated_marker_fns::<C>(registration.app);
    ensure_interpolation_registry(registration.app);
    let mut registry = registration
        .app
        .world_mut()
        .resource_mut::<InterpolationRegistry>();
    registry.set_interpolation::<C>(interpolation_fn);
    registration
}

fn register_interpolation_diff_fn_impl<'a, C>(
    registration: ComponentRegistration<'a, C>,
    interpolation_fn: LerpFn<C>,
) -> ComponentRegistration<'a, C>
where
    C: SyncComponent + RepliconDiffable,
{
    register_interpolated_diff_marker_fns::<C>(registration.app);
    ensure_interpolation_registry(registration.app);
    let mut registry = registration
        .app
        .world_mut()
        .resource_mut::<InterpolationRegistry>();
    registry.set_interpolation::<C>(interpolation_fn);
    registration
}

fn add_interpolation_with_impl<'a, C>(
    registration: ComponentRegistration<'a, C>,
    interpolation_fn: LerpFn<C>,
) -> ComponentRegistration<'a, C>
where
    C: SyncComponent,
{
    let registration = register_interpolation_fn_impl(registration, interpolation_fn);
    add_prepare_interpolation_systems::<C>(registration.app);
    add_interpolation_systems::<C>(registration.app);
    mark_interpolated::<C>(registration.app);
    registration
}

fn add_interpolation_diff_with_impl<'a, C>(
    registration: ComponentRegistration<'a, C>,
    interpolation_fn: LerpFn<C>,
) -> ComponentRegistration<'a, C>
where
    C: SyncComponent + RepliconDiffable,
{
    let registration = register_interpolation_diff_fn_impl(registration, interpolation_fn);
    add_prepare_interpolation_diff_systems::<C>(registration.app);
    add_interpolation_systems::<C>(registration.app);
    mark_interpolated::<C>(registration.app);
    registration
}

fn add_custom_interpolation_impl<C>(
    registration: ComponentRegistration<'_, C>,
) -> ComponentRegistration<'_, C>
where
    C: SyncComponent,
{
    let kind = ComponentKind::of::<C>();
    register_interpolated_marker_fns::<C>(registration.app);
    ensure_interpolation_registry(registration.app);
    let mut registry = registration
        .app
        .world_mut()
        .resource_mut::<InterpolationRegistry>();
    registry
        .interpolation_map
        .entry(kind)
        .and_modify(|r| r.custom_interpolation = true)
        .or_insert_with(|| InterpolationMetadata {
            interpolation: None,
            custom_interpolation: true,
        });
    add_prepare_interpolation_systems::<C>(registration.app);
    mark_interpolated::<C>(registration.app);
    registration
}

fn add_custom_interpolation_diff_impl<C>(
    registration: ComponentRegistration<'_, C>,
) -> ComponentRegistration<'_, C>
where
    C: SyncComponent + RepliconDiffable,
{
    let kind = ComponentKind::of::<C>();
    register_interpolated_diff_marker_fns::<C>(registration.app);
    ensure_interpolation_registry(registration.app);
    let mut registry = registration
        .app
        .world_mut()
        .resource_mut::<InterpolationRegistry>();
    registry
        .interpolation_map
        .entry(kind)
        .and_modify(|r| r.custom_interpolation = true)
        .or_insert_with(|| InterpolationMetadata {
            interpolation: None,
            custom_interpolation: true,
        });
    add_prepare_interpolation_diff_systems::<C>(registration.app);
    mark_interpolated::<C>(registration.app);
    registration
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
    let mut new_history = None;
    insert_interpolation_history_value(entity, &mut new_history, tick, component);
    if let Some(history) = new_history {
        entity.insert(history);
    }
    Ok(())
}

fn write_history_diff<C: SyncComponent + RepliconDiffable>(
    ctx: &mut WriteCtx,
    _rule_fns: &RuleFns<C>,
    entity: &mut DeferredEntity,
    message: &mut Bytes,
) -> bevy_ecs::error::Result<()> {
    let mut new_history = None;
    let Some((tick, diff)) = client_diff_and_tick::<C>(ctx, entity, message)? else {
        return Ok(());
    };
    match diff {
        ComponentDelta::Snapshot {
            index,
            mut component,
        } => {
            C::map_entities(&mut component, ctx);
            let receiver = ctx.get_or_default::<ConfirmedHistoryPatchReceiver<C>>();
            receiver.record_cursor(tick, Some(index));
            insert_interpolation_history_value(entity, &mut new_history, tick, component);
        }
        ComponentDelta::Diffs {
            index,
            diffs: patches,
        } => {
            let receiver = ctx.get_or_default::<ConfirmedHistoryPatchReceiver<C>>();
            receiver.queue_patch_diff(tick, index, patches)?;
        }
    }

    while let Some((tick, value)) = {
        let receiver = ctx.get_or_default::<ConfirmedHistoryPatchReceiver<C>>();
        if let Some(history) = new_history.as_ref() {
            receiver.take_ready_update(history)?
        } else {
            entity
                .get::<ConfirmedHistory<C>>()
                .map(|history| receiver.take_ready_update(history))
                .transpose()?
                .flatten()
        }
    } {
        insert_interpolation_history_value(entity, &mut new_history, tick, value);
    }

    if let Some(history) = new_history {
        entity.insert(history);
    }
    Ok(())
}

fn insert_interpolation_history_value<C: SyncComponent>(
    entity: &mut DeferredEntity,
    new_history: &mut Option<ConfirmedHistory<C>>,
    tick: Tick,
    value: C,
) {
    if let Some(mut history) = entity.get_mut::<ConfirmedHistory<C>>() {
        history.insert_present(tick, value);
    } else {
        let history = new_history.get_or_insert_with(ConfirmedHistory::<C>::default);
        history.insert_present(tick, value);
    }
}

/// Decode the raw Replicon diff bytes and map the Replicon message tick to the
/// corresponding Lightyear server tick.
fn client_diff_and_tick<C: SyncComponent + RepliconDiffable>(
    ctx: &mut WriteCtx,
    entity: &mut DeferredEntity,
    message: &mut Bytes,
) -> bevy_ecs::error::Result<Option<(Tick, ComponentDelta<C>)>> {
    let diff: ComponentDelta<C> = postcard_utils::from_buf(message)?;
    let checkpoints = {
        // SAFETY: we only access resources, which don't alias with the DeferredEntity's component access.
        let world = unsafe { entity.world_mut() };
        let checkpoints =
            world.resource::<ReplicationCheckpointMap>() as *const ReplicationCheckpointMap;
        unsafe { &*checkpoints }
    };
    let Some(tick) = resolve_message_tick(checkpoints, ctx.message_tick) else {
        error!(
            message_tick = ?ctx.message_tick,
            "missing authoritative checkpoint mapping while writing diff interpolation history"
        );
        debug_assert!(
            false,
            "missing authoritative checkpoint mapping while writing diff interpolation history"
        );
        return Ok(None);
    };
    Ok(Some((tick, diff)))
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

fn remove_history_diff<C: SyncComponent + RepliconDiffable>(
    ctx: &mut RemoveCtx,
    entity: &mut DeferredEntity,
) {
    remove_history::<C>(ctx, entity);
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;
    use bevy_app::App;
    use bevy_ecs::component::Component;
    use bevy_replicon::postcard_utils;
    use bevy_replicon::prelude::{RepliconPlugins, RepliconTick, RuleFns};
    use bevy_replicon::shared::replication::diff::diff_index::DiffIndex;
    use bevy_replicon::shared::replication::registry::ReplicationRegistry;
    use bevy_replicon::shared::replication::registry::test_fns::TestFnsEntityExt;
    use bevy_state::app::StatesPlugin;
    use lightyear_replication::registry::replication::AppComponentExt;
    use serde::{Deserialize, Serialize};

    #[derive(Component, Clone, Debug, PartialEq)]
    struct TestComp(f32);

    fn lerp(start: TestComp, end: TestComp, t: f32) -> TestComp {
        TestComp(start.0 + (end.0 - start.0) * t)
    }

    fn diff_lerp(start: TestDiffComponent, end: TestDiffComponent, t: f32) -> TestDiffComponent {
        if t < 0.5 { start } else { end }
    }

    #[derive(Component, Clone, Debug, Deserialize, PartialEq, Serialize)]
    struct TestDiffComponent(u32);

    impl RepliconDiffable for TestDiffComponent {
        type Diff = u32;

        fn apply_diff(&mut self, patch: &Self::Diff) -> bevy_ecs::error::Result<()> {
            self.0 = *patch;
            Ok(())
        }
    }

    fn registry() -> InterpolationRegistry {
        let mut registry = InterpolationRegistry::default();
        registry.set_interpolation::<TestComp>(lerp);
        registry
    }

    #[derive(Serialize)]
    enum TestComponentDelta<'a> {
        Snapshot {
            index: DiffIndex,
            component: &'a TestDiffComponent,
        },
        Diffs {
            index: DiffIndex,
            diffs: &'a [u32],
        },
    }

    fn diff_snapshot(index: u16, component: TestDiffComponent) -> Bytes {
        let mut message = Vec::new();
        let wire = TestComponentDelta::Snapshot {
            index: DiffIndex::new(index),
            component: &component,
        };
        postcard_utils::to_extend_mut(&wire, &mut message).unwrap();
        message.into()
    }

    fn diff_patches(index: u16, patches: &[u32]) -> Bytes {
        let mut message = Vec::new();
        let wire = TestComponentDelta::Diffs {
            index: DiffIndex::new(index),
            diffs: patches,
        };
        postcard_utils::to_extend_mut(&wire, &mut message).unwrap();
        message.into()
    }

    fn setup_interpolation_diff_app() -> (App, bevy_replicon::shared::replication::registry::FnsId)
    {
        let mut app = App::new();
        app.add_plugins((
            StatesPlugin,
            RepliconPlugins,
            crate::plugin::InterpolationPlugin,
        ));
        app.insert_resource(ReplicationCheckpointMap::default());
        app.component::<TestDiffComponent>()
            .replicate_diff()
            .add_custom_interpolation_diff();

        let fns_id =
            app.world_mut()
                .resource_scope(|world, mut registry: Mut<ReplicationRegistry>| {
                    let (_, fns_id) =
                        registry.register_rule_fns(world, RuleFns::<TestDiffComponent>::new_diff());
                    fns_id
                });
        (app, fns_id)
    }

    #[test]
    fn add_interpolation_diff_with_registers_diff_history_and_sampler() {
        let mut app = App::new();
        app.add_plugins((
            StatesPlugin,
            RepliconPlugins,
            crate::plugin::InterpolationPlugin,
        ));
        app.component::<TestDiffComponent>()
            .replicate_diff()
            .add_interpolation_diff_with(diff_lerp);

        let registry = app.world().resource::<InterpolationRegistry>();
        assert!(registry.interpolated::<TestDiffComponent>());
        assert!(registry.has_interpolation_fn::<TestDiffComponent>());
    }

    fn record_checkpoint(app: &mut App, tick: u32) -> RepliconTick {
        let replicon_tick = RepliconTick::new(tick);
        app.world_mut()
            .resource_mut::<ReplicationCheckpointMap>()
            .record(replicon_tick, Tick(tick));
        replicon_tick
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

    #[test]
    fn diff_interpolation_buffers_newer_patch_until_older_base_arrives() {
        let (mut app, fns_id) = setup_interpolation_diff_app();
        let tick0 = record_checkpoint(&mut app, 0);
        let tick3 = record_checkpoint(&mut app, 3);
        let tick5 = record_checkpoint(&mut app, 5);

        let entity = app.world_mut().spawn(Interpolated).id();

        app.world_mut().entity_mut(entity).apply_write(
            diff_snapshot(0, TestDiffComponent(0)),
            fns_id,
            tick0,
        );

        app.world_mut()
            .entity_mut(entity)
            .apply_write(diff_patches(5, &[4, 5]), fns_id, tick5);
        {
            let entity_ref = app.world().entity(entity);
            let history = entity_ref
                .get::<ConfirmedHistory<TestDiffComponent>>()
                .unwrap();
            assert!(history.get_state_at(Tick(5)).is_none());
        }

        app.world_mut()
            .entity_mut(entity)
            .apply_write(diff_patches(3, &[1, 2, 3]), fns_id, tick3);

        let entity_ref = app.world().entity(entity);
        let history = entity_ref
            .get::<ConfirmedHistory<TestDiffComponent>>()
            .unwrap();
        assert_eq!(
            history.get_state_at(Tick(3)).and_then(HistoryState::value),
            Some(&TestDiffComponent(3))
        );
        assert_eq!(
            history.get_state_at(Tick(5)).and_then(HistoryState::value),
            Some(&TestDiffComponent(5))
        );
    }
}
