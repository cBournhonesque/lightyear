use crate::SyncComponent;
use crate::interpolation_history::ConfirmedHistory;
use crate::plugin::{
    add_interpolation_systems, add_prepare_interpolation_systems,
    add_prepare_interpolation_systems_with_diff_metadata,
    add_prepare_interpolation_systems_with_metadata,
};
use alloc::format;
use bevy_ecs::{component::Component, resource::Resource};
use bevy_math::{
    Curve,
    curve::{Ease, EaseFunction, EasingCurve},
};
use bevy_platform::collections::HashMap;
use bevy_platform::collections::HashSet;
use bevy_replicon::bytes::Bytes;
use bevy_replicon::postcard_utils;
use bevy_replicon::prelude::{
    AppMarkerExt, Diffable as RepliconDiffable, PatchIndex, RepliconTick, RuleFns,
};
use bevy_replicon::shared::replication::deferred_entity::DeferredEntity;
use bevy_replicon::shared::replication::diff::{DiffReceiver, DiffWire};
use bevy_replicon::shared::replication::receive_markers::MarkerConfig;
use bevy_replicon::shared::replication::registry::ctx::{RemoveCtx, WriteCtx};
use bevy_replicon::shared::replication::registry::receive_fns::{RemoveFn, WriteFn};
use lightyear_core::interpolation::Interpolated;
use lightyear_core::prelude::Tick;
use lightyear_replication::checkpoint::ReplicationCheckpointMap;
use lightyear_replication::registry::replication::ComponentRegistration;
use lightyear_replication::registry::{ComponentKind, ComponentRegistry, LerpFn};
use tracing::error;

const DIFF_CURSOR_RETENTION: PatchIndex = 10;

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
}

fn register_interpolated_marker_fns<C: SyncComponent>(app: &mut bevy_app::App) {
    register_interpolated_marker_fns_with::<C>(app, write_history::<C>, remove_history::<C, ()>);
}

fn register_interpolated_marker_fns_with<C: SyncComponent>(
    app: &mut bevy_app::App,
    write: WriteFn<C>,
    remove: RemoveFn,
) {
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
    app.set_marker_fns::<Interpolated, C>(write, remove);
    app.world_mut()
        .resource_mut::<InterpolatedMarkerFnRegistry>()
        .kinds
        .insert(kind);
}

fn resolve_message_tick(
    checkpoints: &ReplicationCheckpointMap,
    tick: RepliconTick,
) -> Option<Tick> {
    checkpoints.get(tick)
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

    /// The remote updates will be stored in a [`ConfirmedHistory<C>`](crate::interpolation_history::ConfirmedHistory) component
    /// but the user has to define the interpolation logic themselves
    /// (`lightyear` won't perform any kind of interpolation)
    fn add_custom_interpolation(self) -> Self
    where
        C: SyncComponent;

    /// Like [`Self::add_custom_interpolation`], but for components replicated with Replicon's patch-based diff mode.
    fn add_custom_interpolation_diff(self) -> Self
    where
        C: SyncComponent + RepliconDiffable;
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
        let registration = add_custom_interpolation_with_receive_fns::<C, ()>(
            self,
            write_history::<C>,
            remove_history::<C, ()>,
        );
        add_prepare_interpolation_systems_with_metadata::<C, ()>(registration.app);
        registration
    }

    fn add_custom_interpolation_diff(self) -> Self
    where
        C: SyncComponent + RepliconDiffable,
    {
        let registration = add_custom_interpolation_with_receive_fns::<C, Option<PatchIndex>>(
            self,
            write_history_diff::<C>,
            remove_history_diff::<C>,
        );
        add_prepare_interpolation_systems_with_diff_metadata::<C>(registration.app);
        registration
    }
}

fn add_custom_interpolation_with_receive_fns<'a, C, M>(
    registration: ComponentRegistration<'a, C>,
    write: WriteFn<C>,
    remove: RemoveFn,
) -> ComponentRegistration<'a, C>
where
    C: SyncComponent,
    M: Default + Clone + Send + Sync + 'static,
{
    let kind = ComponentKind::of::<C>();
    register_interpolated_marker_fns_with::<C>(registration.app, write, remove);
    if !registration
        .app
        .world()
        .contains_resource::<InterpolationRegistry>()
    {
        registration
            .app
            .world_mut()
            .insert_resource(InterpolationRegistry::default());
    }
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
    let mut registry = registration
        .app
        .world_mut()
        .resource_mut::<ComponentRegistry>();
    registry
        .component_metadata_map
        .get_mut(&ComponentKind::of::<C>())
        .unwrap()
        .replication
        .as_mut()
        .unwrap()
        .set_interpolated(true);
    registration
}

// TODO: ideally we would update the LastConfirmedTick at this point?
/// Instead of writing into a component directly, it writes data into [`ConfirmedHistory<C>`].
fn write_history<C: SyncComponent>(
    ctx: &mut WriteCtx,
    rule_fns: &RuleFns<C>,
    entity: &mut DeferredEntity,
    message: &mut Bytes,
) -> bevy_ecs::error::Result<()> {
    let Some(component) = interpolation_history_component(ctx, rule_fns, entity, message)? else {
        return Ok(());
    };
    push_confirmed_history(ctx, entity, component, ())
}

fn interpolation_history_component<C: SyncComponent>(
    ctx: &mut WriteCtx,
    rule_fns: &RuleFns<C>,
    entity: &mut DeferredEntity,
    message: &mut Bytes,
) -> bevy_ecs::error::Result<Option<C>> {
    rule_fns.deserialize(ctx, message).map(Some)
}

fn write_history_diff<C: SyncComponent + RepliconDiffable>(
    ctx: &mut WriteCtx,
    _rule_fns: &RuleFns<C>,
    entity: &mut DeferredEntity,
    message: &mut Bytes,
) -> bevy_ecs::error::Result<()> {
    let Some((cursor, component)) =
        interpolation_history_component_diff::<C>(ctx, entity, message)?
    else {
        return Ok(());
    };
    push_confirmed_history(ctx, entity, component, cursor)
}

fn interpolation_history_component_diff<C: SyncComponent + RepliconDiffable>(
    ctx: &mut WriteCtx,
    entity: &mut DeferredEntity,
    message: &mut Bytes,
) -> bevy_ecs::error::Result<Option<(Option<PatchIndex>, C)>> {
    let wire: DiffWire<C, C::Patch> = postcard_utils::from_buf(message)?;
    let (cursor, value) = match wire {
        DiffWire::Snapshot { cursor, mut value } => {
            C::map_entities(&mut value, ctx);
            entity.insert(DiffReceiver::<C>::new(cursor));
            (cursor, value)
        }
        DiffWire::Patches {
            first_patch_index,
            patches,
        } => {
            if patches.is_empty() {
                return Ok(None);
            }
            let base_cursor = first_patch_index.checked_sub(1);
            let cursor = Some(first_patch_index + patches.len() as PatchIndex - 1);
            let live_receiver_cursor = entity
                .get::<DiffReceiver<C>>()
                .map(|receiver| receiver.last_applied());
            let live_is_base = live_receiver_cursor == Some(base_cursor);
            let has_history = entity
                .get::<ConfirmedHistory<C, Option<PatchIndex>>>()
                .is_some();
            let has_live_component = entity.get::<C>().is_some();
            let mut value = entity
                .get::<ConfirmedHistory<C, Option<PatchIndex>>>()
                .and_then(|history| history.value_with_metadata(&base_cursor).cloned())
                .or_else(|| {
                    live_is_base
                        .then(|| entity.get::<C>().map(|value| value.clone()))
                        .flatten()
                })
                .ok_or_else(|| {
                    format!(
                        "received diff patches for `{}` without a confirmed base: base_cursor={:?}, cursor={:?}, batch_count={}, live_receiver_cursor={:?}, has_history={}, has_live_component={}",
                        core::any::type_name::<C>(),
                        base_cursor,
                        cursor,
                        patches.len(),
                        live_receiver_cursor,
                        has_history,
                        has_live_component,
                    )
                })?;
            for batch in patches {
                for patch in batch.iter() {
                    value.apply_patch(patch)?;
                }
            }
            if let Some(mut history) = entity.get_mut::<ConfirmedHistory<C, Option<PatchIndex>>>() {
                history.prune_metadata_before_cursor(
                    first_patch_index.saturating_sub(DIFF_CURSOR_RETENTION),
                );
            }
            (cursor, value)
        }
    };

    Ok(Some((cursor, value)))
}

fn push_confirmed_history<C, M>(
    ctx: &mut WriteCtx,
    entity: &mut DeferredEntity,
    component: C,
    metadata: M,
) -> bevy_ecs::error::Result<()>
where
    C: SyncComponent,
    M: Default + Clone + Send + Sync + 'static,
{
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
    if let Some(mut history) = entity.get_mut::<ConfirmedHistory<C, M>>() {
        history.push_with_metadata(tick, component, metadata);
    } else {
        let mut history = ConfirmedHistory::<C, M>::default();
        history.push_with_metadata(tick, component, metadata);
        entity.insert(history);
    }
    Ok(())
}

/// Records a component removal in `ConfirmedHistory<C>`.
///
/// The live component is removed later by interpolation systems once the interpolation timeline
/// reaches the server tick that produced this removal.
fn remove_history<C, M>(ctx: &mut RemoveCtx, entity: &mut DeferredEntity)
where
    C: Component,
    M: Default + Send + Sync + 'static,
{
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
    if let Some(mut history) = entity.get_mut::<ConfirmedHistory<C, M>>() {
        history.push_remove_with_metadata(tick, M::default());
    } else {
        let mut history = ConfirmedHistory::<C, M>::default();
        history.push_remove_with_metadata(tick, M::default());
        entity.insert(history);
    }
}

fn remove_history_diff<C: Component + RepliconDiffable>(
    ctx: &mut RemoveCtx,
    entity: &mut DeferredEntity,
) {
    entity.remove::<DiffReceiver<C>>();
    remove_history::<C, Option<PatchIndex>>(ctx, entity);
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use alloc::vec::Vec;
    use bevy_app::App;
    use bevy_ecs::prelude::{Component, Entity, Mut};
    use bevy_replicon::postcard_utils;
    use bevy_replicon::prelude::{RepliconPlugins, RuleFns};
    use bevy_replicon::shared::replication::diff::DiffWire;
    use bevy_replicon::shared::replication::receive_markers::MarkerConfig;
    use bevy_replicon::shared::replication::registry::{
        FnsId, ReplicationRegistry, test_fns::TestFnsEntityExt,
    };
    use bevy_state::app::StatesPlugin;
    use serde::{Deserialize, Serialize};

    #[derive(Component, Clone, Debug, Deserialize, PartialEq, Serialize)]
    struct DiffTestValue(u32);

    impl RepliconDiffable for DiffTestValue {
        type Patch = u32;

        fn apply_patch(&mut self, patch: &Self::Patch) -> bevy_ecs::error::Result<()> {
            self.0 = *patch;
            Ok(())
        }
    }

    #[test]
    fn resolve_message_tick_uses_authoritative_tick_for_large_replicon_gap() {
        let mut checkpoints = ReplicationCheckpointMap::default();
        checkpoints.record(RepliconTick::new(200), Tick(20));

        assert_eq!(
            resolve_message_tick(&checkpoints, RepliconTick::new(200)),
            Some(Tick(20))
        );
    }

    #[test]
    fn resolve_message_tick_collapses_multiple_replicon_ticks_for_same_authoritative_tick() {
        let mut checkpoints = ReplicationCheckpointMap::default();
        checkpoints.record(RepliconTick::new(100), Tick(20));
        checkpoints.record(RepliconTick::new(101), Tick(20));

        assert_eq!(
            resolve_message_tick(&checkpoints, RepliconTick::new(100)),
            Some(Tick(20))
        );
        assert_eq!(
            resolve_message_tick(&checkpoints, RepliconTick::new(101)),
            Some(Tick(20))
        );
    }

    #[test]
    fn interpolation_diff_records_older_subset_after_cumulative_patch() {
        let (mut app, fns_id) = setup_diff_interpolation_app();
        record_checkpoints(&mut app);

        let entity = app.world_mut().spawn(Interpolated).id();

        apply_diff_write(
            &mut app,
            entity,
            fns_id,
            RepliconTick::new(10),
            DiffWire::Snapshot {
                cursor: None,
                value: DiffTestValue(0),
            },
        );
        apply_diff_write(
            &mut app,
            entity,
            fns_id,
            RepliconTick::new(12),
            DiffWire::Patches {
                first_patch_index: 0,
                patches: vec![vec![1], vec![2]],
            },
        );
        apply_diff_write(
            &mut app,
            entity,
            fns_id,
            RepliconTick::new(11),
            DiffWire::Patches {
                first_patch_index: 0,
                patches: vec![vec![1]],
            },
        );

        let history = app
            .world()
            .entity(entity)
            .get::<ConfirmedHistory<DiffTestValue, Option<PatchIndex>>>()
            .unwrap();
        assert_eq!(
            history
                .get_nth(0)
                .map(|(tick, value)| (tick, value.clone())),
            Some((Tick(0), DiffTestValue(0)))
        );
        assert_eq!(
            history
                .get_nth(1)
                .map(|(tick, value)| (tick, value.clone())),
            Some((Tick(1), DiffTestValue(1)))
        );
        assert_eq!(
            history
                .get_nth(2)
                .map(|(tick, value)| (tick, value.clone())),
            Some((Tick(2), DiffTestValue(2)))
        );
        assert_eq!(
            history.value_with_metadata(&Some(0)).cloned(),
            Some(DiffTestValue(1))
        );
    }

    #[test]
    fn interpolation_diff_prunes_bases_older_than_cursor_window() {
        let (mut app, fns_id) = setup_diff_interpolation_app();
        {
            let mut checkpoints = app.world_mut().resource_mut::<ReplicationCheckpointMap>();
            for tick in 10..=37 {
                checkpoints.record(RepliconTick::new(tick), Tick(tick - 10));
            }
        }

        let entity = app.world_mut().spawn(Interpolated).id();
        apply_diff_write(
            &mut app,
            entity,
            fns_id,
            RepliconTick::new(10),
            DiffWire::Snapshot {
                cursor: None,
                value: DiffTestValue(0),
            },
        );
        for patch_index in 0..=26 {
            apply_diff_write(
                &mut app,
                entity,
                fns_id,
                RepliconTick::new(11 + patch_index),
                DiffWire::Patches {
                    first_patch_index: patch_index as PatchIndex,
                    patches: vec![vec![patch_index + 1]],
                },
            );
        }

        let history = app
            .world()
            .entity(entity)
            .get::<ConfirmedHistory<DiffTestValue, Option<PatchIndex>>>()
            .unwrap();
        assert!(
            history.value_with_metadata(&None).is_none(),
            "pre-patch base should be pruned once the received patch window starts at 26"
        );
        assert!(
            history.value_with_metadata(&Some(0)).is_none(),
            "cursor 0 is older than 26 - 10"
        );
        assert_eq!(
            history.value_with_metadata(&Some(16)).cloned(),
            Some(DiffTestValue(17))
        );
        assert_eq!(
            history.value_with_metadata(&Some(26)).cloned(),
            Some(DiffTestValue(27))
        );
    }

    fn setup_diff_interpolation_app() -> (App, FnsId) {
        let mut app = App::new();
        app.add_plugins((StatesPlugin, RepliconPlugins));
        app.insert_resource(ReplicationCheckpointMap::default());
        app.register_marker_with::<Interpolated>(MarkerConfig {
            priority: 100,
            need_history: true,
        });
        app.set_marker_fns::<Interpolated, DiffTestValue>(
            write_history_diff::<DiffTestValue>,
            remove_history_diff::<DiffTestValue>,
        );
        let (_, fns_id) =
            app.world_mut()
                .resource_scope(|world, mut registry: Mut<ReplicationRegistry>| {
                    registry.register_rule_fns(world, RuleFns::<DiffTestValue>::new_diff())
                });
        (app, fns_id)
    }

    fn record_checkpoints(app: &mut App) {
        let mut checkpoints = app.world_mut().resource_mut::<ReplicationCheckpointMap>();
        checkpoints.record(RepliconTick::new(10), Tick(0));
        checkpoints.record(RepliconTick::new(11), Tick(1));
        checkpoints.record(RepliconTick::new(12), Tick(2));
    }

    fn apply_diff_write(
        app: &mut App,
        entity: Entity,
        fns_id: FnsId,
        message_tick: RepliconTick,
        wire: DiffWire<DiffTestValue, u32>,
    ) {
        let mut message = Vec::new();
        postcard_utils::to_extend_mut(&wire, &mut message).unwrap();
        app.world_mut()
            .entity_mut(entity)
            .apply_write(message, fns_id, message_tick);
    }
}
