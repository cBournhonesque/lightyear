use crate::SyncComponent;
use crate::archetypes::InterpolationWorld;
use crate::registry::{InterpolationRegistry, sample_history_with_interpolation};
use crate::rules::{
    ApplyInterpolationContext, CachedInterpolationComponent, InterpolationRuleId,
    UpdateHistoryContext,
};
use crate::timeline::InterpolationTimeline;
use bevy_ecs::archetype::Archetype;
use bevy_ecs::component::StorageType;
use bevy_ecs::prelude::*;
use bevy_ecs::storage::Table;
use bevy_ecs::world::unsafe_world_cell::UnsafeWorldCell;
use bevy_replicon::shared::replication::diff::Diffable as RepliconDiffable;
use bevy_replicon::shared::replication::storage::ReplicationStorage;
use bevy_utils::prelude::DebugName;
use core::cell::UnsafeCell;
use lightyear_core::history_buffer::HistoryState;
use lightyear_core::prelude::{ConfirmedHistory, Interpolated, NetworkTimeline};
use lightyear_core::tick::Tick;
use lightyear_replication::checkpoint::ReplicationCheckpointMap;
use lightyear_replication::deferred_entity::DeferredEntityCommands;
use lightyear_replication::diff_history::HistoryDiffReceiver;
#[allow(unused_imports)]
use tracing::{info, trace};

/// Compute the interpolation fraction
pub fn interpolation_fraction(start: Tick, end: Tick, current: Tick, overstep: f32) -> f32 {
    ((current - start) as f32 + overstep) / (end - start) as f32
}

/// Updates interpolation histories and component presence.
///
/// This is intentionally archetype-driven: a local
/// [`InterpolatedArchetypes`](crate::archetypes::InterpolatedArchetypes) cache
/// stores the highest-priority matching rule per archetype/component and the
/// type-erased history functions that should run for each cached archetype.
///
/// Component insertion/removal happens here so custom interpolation systems that
/// run after [`crate::plugin::InterpolationSystems::Prepare`] see the live
/// component set matching the interpolation timeline.
pub(crate) fn update_interpolation_history(
    mut interpolation_world: InterpolationWorld,
    clients: Query<&InterpolationTimeline, Without<Interpolated>>,
    interpolation_registry: Res<InterpolationRegistry>,
    checkpoints: Res<ReplicationCheckpointMap>,
    mut replication_storage: Option<ResMut<ReplicationStorage>>,
    mut commands: Commands,
) {
    // TODO: handle multiple interpolation timelines
    // TODO: exclude host-server
    let Ok(timeline) = clients.single() else {
        return;
    };
    let current_interpolate_tick = timeline.now().tick();
    let interpolation_overstep = timeline.overstep().to_f32();
    let server_complete_tick = checkpoints.last_confirmed_tick();

    let mut deferred_apply = DeferredEntityCommands::default();

    interpolation_world.update_archetypes(&interpolation_registry);
    let world = interpolation_world.world;
    for (archetype, cached_archetype) in interpolation_world.iter_archetypes() {
        for component in cached_archetype.history_update_components() {
            let ctx = UpdateHistoryContext {
                server_complete_tick,
                current_interpolate_tick,
                interpolation_overstep,
                interpolation: component.interpolation(),
            };
            (component.update_history())(
                world,
                archetype,
                component,
                ctx,
                replication_storage.as_deref_mut(),
                &mut deferred_apply,
            );
        }
    }

    deferred_apply.apply(&mut commands);
}

/// Applies Lightyear-owned interpolation values to components already present
/// at the interpolation timeline.
///
/// This runs after [`update_interpolation_history`] and after its deferred
/// component insertions/removals have been flushed.
pub(crate) fn apply_interpolation(
    mut interpolation_world: InterpolationWorld,
    clients: Query<&InterpolationTimeline, Without<Interpolated>>,
    interpolation_registry: Res<InterpolationRegistry>,
) {
    // TODO: handle multiple interpolation timelines
    // TODO: exclude host-server
    let Ok(timeline) = clients.single() else {
        return;
    };
    let current_interpolate_tick = timeline.now().tick();
    let interpolation_overstep = timeline.overstep().to_f32();
    let ctx = ApplyInterpolationContext {
        interpolation_tick: current_interpolate_tick,
        interpolation_overstep,
    };

    interpolation_world.update_archetypes(&interpolation_registry);
    let world = interpolation_world.world;
    for (archetype, cached_archetype) in interpolation_world.iter_archetypes() {
        for component in cached_archetype.apply_rules() {
            (component.apply_interpolation())(
                world,
                archetype,
                &interpolation_registry,
                component.rule_id(),
                ctx,
            );
        }
    }
}

pub(crate) fn update_history_archetype_erased<C: Component + Clone>(
    world: UnsafeWorldCell,
    archetype: &Archetype,
    component: &CachedInterpolationComponent,
    ctx: UpdateHistoryContext,
    _replication_storage: Option<&mut ReplicationStorage>,
    deferred_apply: &mut DeferredEntityCommands,
) {
    let StorageType::Table = component.history_storage() else {
        debug_assert!(
            false,
            "ConfirmedHistory components are expected to use table storage"
        );
        return;
    };
    let Some(table) = table_for_archetype(world, archetype) else {
        return;
    };
    let Some(histories) = (unsafe {
        table.get_data_slice_for::<ConfirmedHistory<C>>(component.history_component_id())
    }) else {
        return;
    };
    let present = component.live_component_present();
    for entity in archetype.entities() {
        let entity_id = entity.id();
        let row = entity.table_row().index();
        let history = unsafe { &mut *histories.get_unchecked(row).get() };
        update_history_inner::<C>(history, entity_id, ctx);
        let sample = sample_history_with_interpolation(
            ctx.interpolation,
            history,
            ctx.current_interpolate_tick,
            ctx.interpolation_overstep,
        );
        queue_history_presence::<C>(deferred_apply, entity_id, present, sample);
    }
}

pub(crate) fn update_history_diff_archetype_erased<C>(
    world: UnsafeWorldCell,
    archetype: &Archetype,
    component: &CachedInterpolationComponent,
    ctx: UpdateHistoryContext,
    replication_storage: Option<&mut ReplicationStorage>,
    deferred_apply: &mut DeferredEntityCommands,
) where
    C: Component + Clone + RepliconDiffable,
{
    let StorageType::Table = component.history_storage() else {
        debug_assert!(
            false,
            "ConfirmedHistory components are expected to use table storage"
        );
        return;
    };
    let Some(table) = table_for_archetype(world, archetype) else {
        return;
    };
    let Some(histories) = (unsafe {
        table.get_data_slice_for::<ConfirmedHistory<C>>(component.history_component_id())
    }) else {
        return;
    };
    let Some(storage) = replication_storage else {
        return;
    };
    let present = component.live_component_present();
    for entity in archetype.entities() {
        let entity_id = entity.id();
        let Some(history_diff_receiver) = storage.get_mut::<HistoryDiffReceiver<C>>(entity_id)
        else {
            continue;
        };
        let row = entity.table_row().index();
        let history = unsafe { &mut *histories.get_unchecked(row).get() };

        if let Some(server_complete_tick) = ctx.server_complete_tick
            && !history_diff_receiver.has_pending_diff_at_tick(server_complete_tick)
            && let Some(previous_newest_tick) = history.push_unchanged(server_complete_tick)
        {
            trace!(
                target: "lightyear_debug::interpolation",
                kind = "confirmed_history_unchanged_advance",
                schedule = "Update",
                sample_point = "Update",
                entity = ?entity_id,
                component = ?DebugName::type_name::<C>(),
                previous_newest_tick = previous_newest_tick.0,
                server_complete_tick = server_complete_tick.0,
                history_len = history.len(),
                "advanced unchanged diff interpolation history"
            );
        }

        if !history_diff_receiver.has_pending_diffs() {
            drain_old_history(history, ctx.current_interpolate_tick);

            if let Some(server_complete_tick) = ctx.server_complete_tick {
                history_diff_receiver.clear_before_tick(server_complete_tick, history);
            }
        }

        let sample = sample_history_with_interpolation(
            ctx.interpolation,
            history,
            ctx.current_interpolate_tick,
            ctx.interpolation_overstep,
        );
        queue_history_presence::<C>(deferred_apply, entity_id, present, sample);
    }
}

fn update_history_inner<C: Component + Clone>(
    history: &mut ConfirmedHistory<C>,
    entity: Entity,
    ctx: UpdateHistoryContext,
) {
    // Replicon's marker fns already ran before this system. If this component received an
    // explicit update or removal at the completed server tick T, `write_history` /
    // `remove_history` already recorded that exact tick and `push_unchanged(T)` returns None.
    //
    // Therefore, when the newest confirmed state is still an Updated value older than T,
    // mutate-message completeness tells us no update/removal for this component occurred
    // through T, so we can carry the newest value forward as unchanged.
    if let Some(server_complete_tick) = ctx.server_complete_tick
        && let Some(previous_newest_tick) = history.push_unchanged(server_complete_tick)
    {
        trace!(
            target: "lightyear_debug::interpolation",
            kind = "confirmed_history_unchanged_advance",
            schedule = "Update",
            sample_point = "Update",
            entity = ?entity,
            component = ?DebugName::type_name::<C>(),
            previous_newest_tick = previous_newest_tick.0,
            server_complete_tick = server_complete_tick.0,
            history_len = history.len(),
            "advanced unchanged interpolation history"
        );
    }

    drain_old_history(history, ctx.current_interpolate_tick);
}

fn drain_old_history<C: Component + Clone>(
    history: &mut ConfirmedHistory<C>,
    current_interpolate_tick: Tick,
) {
    // Smart drain: only pop when there are 3+ keyframes and the second-oldest
    // has already been passed. This keeps a [behind, newest] pair alive during
    // short loss gaps instead of collapsing immediately to a single keyframe.
    while history.len() >= 3
        && history
            .get_nth_tick(1)
            .is_some_and(|tick| tick <= current_interpolate_tick)
    {
        history.pop_present();
    }
}

fn queue_history_presence<C: Component + Clone>(
    deferred_apply: &mut DeferredEntityCommands,
    entity: Entity,
    present: bool,
    sample: Option<HistoryState<C>>,
) {
    // Apply the history state for the current interpolation time to the live component set:
    // insert once the add/update tick becomes visible, remove once a removal tick is reached,
    // and otherwise leave the current component value alone.
    match sample {
        None | Some(HistoryState::Removed) if present => {
            deferred_apply.remove::<C>(entity);
        }
        Some(HistoryState::Updated(value)) if !present => {
            deferred_apply.insert(entity, value);
        }
        _ => {}
    }
}

#[derive(Clone, Copy)]
enum ComponentColumn<'w, C> {
    Table(&'w [UnsafeCell<C>]),
    Missing,
    NonTable,
}

fn table_for_archetype<'w>(world: UnsafeWorldCell<'w>, archetype: &Archetype) -> Option<&'w Table> {
    unsafe { world.storages().tables.get(archetype.table_id()) }
}

fn live_component_column<'w, C: Component>(
    world: UnsafeWorldCell<'w>,
    archetype: &Archetype,
    table: &'w Table,
) -> ComponentColumn<'w, C> {
    let Some(component_id) = world.components().component_id::<C>() else {
        return ComponentColumn::Missing;
    };
    if !archetype.contains(component_id) {
        return ComponentColumn::Missing;
    }
    let Some(StorageType::Table) = archetype.get_storage_type(component_id) else {
        return ComponentColumn::NonTable;
    };
    unsafe {
        table
            .get_data_slice_for::<C>(component_id)
            .map_or(ComponentColumn::NonTable, ComponentColumn::Table)
    }
}

/// Applies one selected component interpolation rule to one archetype.
pub(crate) fn apply_interpolation_archetype_erased<C: SyncComponent>(
    world: UnsafeWorldCell,
    archetype: &Archetype,
    interpolation_registry: &InterpolationRegistry,
    rule_id: InterpolationRuleId,
    ctx: ApplyInterpolationContext,
) {
    let Some(history_component_id) = world.components().component_id::<ConfirmedHistory<C>>()
    else {
        return;
    };
    if !archetype.contains(history_component_id) {
        return;
    }
    let Some(StorageType::Table) = archetype.get_storage_type(history_component_id) else {
        debug_assert!(
            false,
            "ConfirmedHistory components are expected to use table storage"
        );
        return;
    };
    let Some(table) = table_for_archetype(world, archetype) else {
        return;
    };
    let Some(histories) =
        (unsafe { table.get_data_slice_for::<ConfirmedHistory<C>>(history_component_id) })
    else {
        return;
    };
    let components = live_component_column::<C>(world, archetype, table);

    for entity in archetype.entities() {
        let row = entity.table_row().index();
        let history = unsafe { &*histories.get_unchecked(row).get() };
        let Some(HistoryState::Updated(interpolated)) = interpolation_registry.sample_for_rule(
            rule_id,
            history,
            ctx.interpolation_tick,
            ctx.interpolation_overstep,
        ) else {
            continue;
        };

        trace!(
            target: "lightyear_debug::interpolation",
            kind = "interpolation_apply",
            schedule = "Update",
            sample_point = "Update",
            component = ?DebugName::type_name::<C>(),
            interpolation_tick = ctx.interpolation_tick.0,
            interpolation_overstep = ctx.interpolation_overstep,
            history_len = history.len(),
            "applied interpolation"
        );
        match components {
            ComponentColumn::Table(components) => {
                let component = unsafe { &mut *components.get_unchecked(row).get() };
                *component = interpolated;
            }
            ComponentColumn::Missing => {}
            ComponentColumn::NonTable => {}
        }
    }
}

pub(crate) fn present_history_bracket<C: Component + Clone>(
    history: &ConfirmedHistory<C>,
    interpolation_tick: Tick,
) -> Option<(Tick, C, Option<(Tick, C)>)> {
    let previous_index = (0..history.len())
        .take_while(|i| {
            history
                .get_nth_tick(*i)
                .is_some_and(|tick| tick <= interpolation_tick)
        })
        .last()?;

    let (start_tick, start_state) = history.get_nth_state(previous_index)?;
    let HistoryState::Updated(start) = start_state else {
        return None;
    };

    let end = match history.get_nth_state(previous_index + 1) {
        Some((end_tick, HistoryState::Updated(end))) => Some((end_tick, end.clone())),
        Some((_, HistoryState::Removed)) => None,
        None => None,
    };

    Some((start_tick, start.clone(), end))
}

// #[cfg(test)]
#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::AppInterpolationExt;
    use crate::registry::{InterpolationRegistry, InterpolationRuleComponentIds};
    use crate::rules::{InterpolationFns, InterpolationRuleConfig};
    use alloc::vec;
    use bevy_app::{App, Update};
    use bevy_ecs::archetype::Archetype;
    use bevy_ecs::component::Component;
    use bevy_ecs::query::{QueryFilter, QueryState};
    use bevy_replicon::prelude::{
        Diffable as RepliconDiffable, RepliconPlugins, RepliconSharedPlugin, RepliconTick,
    };
    use bevy_replicon::shared::replication::diff::diff_index::DiffIndex;
    use bevy_state::app::StatesPlugin;
    use bevy_time::TimePlugin;
    use core::sync::atomic::{AtomicUsize, Ordering};
    use lightyear_core::prelude::Interpolated;
    use lightyear_core::time::TickInstant;
    use lightyear_replication::checkpoint::ReplicationCheckpointMap;
    use lightyear_replication::diff_history::HistoryDiffReceiver;
    use lightyear_replication::registry::replication::AppComponentExt;
    use lightyear_sync::prelude::client::IsSynced;
    use serde::{Deserialize, Serialize};

    #[derive(Component, Clone, Debug, Deserialize, PartialEq, Serialize)]
    struct TestComp(f32);

    #[derive(Component, Clone, Debug, Deserialize, PartialEq, Serialize)]
    struct TestComp2(f32);

    #[derive(Component, Clone, Debug, Deserialize, PartialEq, Serialize)]
    struct TestBundleComp<const N: usize>(f32);

    #[derive(Component)]
    struct SmoothRule;

    #[derive(Component)]
    struct HistoryOnlyRule;

    static BUNDLE2_PRIORITY_CALLS: AtomicUsize = AtomicUsize::new(0);
    static BUNDLE3_PRIORITY_CALLS: AtomicUsize = AtomicUsize::new(0);

    impl RepliconDiffable for TestComp {
        type Diff = f32;

        fn apply_diff(&mut self, diff: &Self::Diff) -> bevy_ecs::error::Result<()> {
            self.0 = *diff;
            Ok(())
        }
    }

    fn idx(value: u16) -> DiffIndex {
        DiffIndex::new(value)
    }

    fn lerp(start: TestComp, end: TestComp, t: f32) -> TestComp {
        TestComp(start.0 + (end.0 - start.0) * t)
    }

    fn marker_lerp(_start: TestComp, _end: TestComp, _t: f32) -> TestComp {
        TestComp(42.0)
    }

    fn bundle_lerp(
        start: (TestComp, TestComp2),
        end: (TestComp, TestComp2),
        t: f32,
    ) -> (TestComp, TestComp2) {
        (
            TestComp(100.0 + start.0.0 + (end.0.0 - start.0.0) * t),
            TestComp2(200.0 + start.1.0 + (end.1.0 - start.1.0) * t),
        )
    }

    fn bundle2_priority_lerp(
        start: (TestComp, TestComp2),
        end: (TestComp, TestComp2),
        t: f32,
    ) -> (TestComp, TestComp2) {
        BUNDLE2_PRIORITY_CALLS.fetch_add(1, Ordering::SeqCst);
        (
            TestComp(100.0 + start.0.0 + (end.0.0 - start.0.0) * t),
            TestComp2(200.0 + start.1.0 + (end.1.0 - start.1.0) * t),
        )
    }

    fn bundle3_priority_lerp(
        start: (TestComp, TestComp2, TestBundleComp<3>),
        end: (TestComp, TestComp2, TestBundleComp<3>),
        t: f32,
    ) -> (TestComp, TestComp2, TestBundleComp<3>) {
        BUNDLE3_PRIORITY_CALLS.fetch_add(1, Ordering::SeqCst);
        (
            TestComp(300.0 + start.0.0 + (end.0.0 - start.0.0) * t),
            TestComp2(400.0 + start.1.0 + (end.1.0 - start.1.0) * t),
            TestBundleComp::<3>(500.0 + start.2.0 + (end.2.0 - start.2.0) * t),
        )
    }

    fn bundle8_lerp(
        start: (
            TestBundleComp<1>,
            TestBundleComp<2>,
            TestBundleComp<3>,
            TestBundleComp<4>,
            TestBundleComp<5>,
            TestBundleComp<6>,
            TestBundleComp<7>,
            TestBundleComp<8>,
        ),
        end: (
            TestBundleComp<1>,
            TestBundleComp<2>,
            TestBundleComp<3>,
            TestBundleComp<4>,
            TestBundleComp<5>,
            TestBundleComp<6>,
            TestBundleComp<7>,
            TestBundleComp<8>,
        ),
        t: f32,
    ) -> (
        TestBundleComp<1>,
        TestBundleComp<2>,
        TestBundleComp<3>,
        TestBundleComp<4>,
        TestBundleComp<5>,
        TestBundleComp<6>,
        TestBundleComp<7>,
        TestBundleComp<8>,
    ) {
        (
            TestBundleComp::<1>(10.0 + start.0.0 + (end.0.0 - start.0.0) * t),
            TestBundleComp::<2>(20.0 + start.1.0 + (end.1.0 - start.1.0) * t),
            TestBundleComp::<3>(30.0 + start.2.0 + (end.2.0 - start.2.0) * t),
            TestBundleComp::<4>(40.0 + start.3.0 + (end.3.0 - start.3.0) * t),
            TestBundleComp::<5>(50.0 + start.4.0 + (end.4.0 - start.4.0) * t),
            TestBundleComp::<6>(60.0 + start.5.0 + (end.5.0 - start.5.0) * t),
            TestBundleComp::<7>(70.0 + start.6.0 + (end.6.0 - start.6.0) * t),
            TestBundleComp::<8>(80.0 + start.7.0 + (end.7.0 - start.7.0) * t),
        )
    }

    fn setup_app(current_tick: Tick, send_interval_ms: u64) -> App {
        let mut app = App::new();
        app.add_plugins((StatesPlugin, RepliconSharedPlugin::default()));
        app.world_mut()
            .insert_resource(ReplicationCheckpointMap::default());
        app.world_mut()
            .insert_resource(ReplicationStorage::default());
        let mut registry = InterpolationRegistry::default();
        let fns = InterpolationFns::interpolate(lerp);
        let component_ids =
            InterpolationRuleComponentIds::for_component::<TestComp>(app.world_mut(), &fns);
        registry.insert_rule::<TestComp, ()>(
            fns,
            InterpolationRuleConfig::default(),
            component_ids,
        );
        app.world_mut().insert_resource(registry);

        let mut timeline = InterpolationTimeline::default();
        timeline.set_now(TickInstant::from(current_tick));
        timeline.remote_send_interval = core::time::Duration::from_millis(send_interval_ms);
        app.world_mut()
            .spawn((timeline, IsSynced::<InterpolationTimeline>::default()));
        app
    }

    fn confirm_server_tick(app: &mut App, replicon_tick: u32, server_tick: Tick) {
        let replicon_tick = RepliconTick::new(replicon_tick);
        let mut checkpoints = app.world_mut().resource_mut::<ReplicationCheckpointMap>();
        checkpoints.record(replicon_tick, server_tick);
        checkpoints.record_last_confirmed_tick(replicon_tick);
    }

    fn set_interpolation_tick(app: &mut App, tick: Tick) {
        let mut timelines = app.world_mut().query::<&mut InterpolationTimeline>();
        let mut timeline = timelines.single_mut(app.world_mut()).unwrap();
        timeline.set_now(TickInstant::from(tick));
    }

    fn insert_confirmed_history(
        app: &mut App,
        entity: Entity,
        history: ConfirmedHistory<TestComp>,
    ) {
        app.world_mut()
            .entity_mut(entity)
            .insert((Interpolated, history));
    }

    fn add_interpolation_test_systems(app: &mut App) {
        crate::plugin::add_update_interpolation_system(app);
    }

    fn two_point_history() -> ConfirmedHistory<TestComp> {
        let mut history = ConfirmedHistory::<TestComp>::default();
        history.insert_present(Tick(10), TestComp(0.0));
        history.insert_present(Tick(20), TestComp(10.0));
        history
    }

    fn two_point_history2() -> ConfirmedHistory<TestComp2> {
        let mut history = ConfirmedHistory::<TestComp2>::default();
        history.insert_present(Tick(10), TestComp2(0.0));
        history.insert_present(Tick(20), TestComp2(10.0));
        history
    }

    fn two_point_bundle_history<const N: usize>() -> ConfirmedHistory<TestBundleComp<N>> {
        let mut history = ConfirmedHistory::<TestBundleComp<N>>::default();
        history.insert_present(Tick(10), TestBundleComp::<N>(0.0));
        history.insert_present(Tick(20), TestBundleComp::<N>(10.0));
        history
    }

    fn use_diff_history_rule(app: &mut App) {
        let fns = InterpolationFns::interpolate(lerp);
        let component_ids =
            InterpolationRuleComponentIds::for_component::<TestComp>(app.world_mut(), &fns);
        app.world_mut()
            .resource_mut::<InterpolationRegistry>()
            .insert_diff_rule::<TestComp, ()>(
                fns,
                InterpolationRuleConfig { priority: 100 },
                component_ids,
            );
    }

    fn insert_rule<C, F>(app: &mut App, fns: InterpolationFns<C>, config: InterpolationRuleConfig)
    where
        C: SyncComponent,
        F: QueryFilter + 'static,
    {
        let component_ids =
            InterpolationRuleComponentIds::for_component::<C>(app.world_mut(), &fns);
        app.world_mut()
            .resource_mut::<InterpolationRegistry>()
            .insert_rule::<C, F>(fns, config, component_ids);
    }

    #[test]
    fn filtered_interpolation_rule_overrides_default_for_matching_archetype() {
        let mut app = setup_app(Tick(15), 40);
        add_interpolation_test_systems(&mut app);
        QueryState::<&Archetype, With<SmoothRule>>::new(app.world_mut());
        insert_rule::<TestComp, With<SmoothRule>>(
            &mut app,
            InterpolationFns::interpolate(marker_lerp),
            InterpolationRuleConfig { priority: 100 },
        );

        let default_entity = app.world_mut().spawn(TestComp(0.0)).id();
        insert_confirmed_history(&mut app, default_entity, two_point_history());
        let filtered_entity = app.world_mut().spawn((TestComp(0.0), SmoothRule)).id();
        insert_confirmed_history(&mut app, filtered_entity, two_point_history());

        app.update();

        assert_eq!(
            app.world().get::<TestComp>(default_entity),
            Some(&TestComp(5.0))
        );
        assert_eq!(
            app.world().get::<TestComp>(filtered_entity),
            Some(&TestComp(42.0))
        );
    }

    #[test]
    fn selected_history_only_rule_suppresses_default_apply() {
        let mut app = setup_app(Tick(15), 40);
        add_interpolation_test_systems(&mut app);
        QueryState::<&Archetype, With<HistoryOnlyRule>>::new(app.world_mut());
        insert_rule::<TestComp, With<HistoryOnlyRule>>(
            &mut app,
            InterpolationFns::history_only_with_interpolator(marker_lerp),
            InterpolationRuleConfig { priority: 100 },
        );

        let entity = app.world_mut().spawn((TestComp(7.0), HistoryOnlyRule)).id();
        insert_confirmed_history(&mut app, entity, two_point_history());

        app.update();

        assert_eq!(app.world().get::<TestComp>(entity), Some(&TestComp(7.0)));
    }

    #[test]
    fn rule_registration_invalidates_cached_archetype_selection() {
        let mut app = setup_app(Tick(15), 40);
        add_interpolation_test_systems(&mut app);
        let entity = app.world_mut().spawn((TestComp(0.0), SmoothRule)).id();
        insert_confirmed_history(&mut app, entity, two_point_history());

        app.update();
        assert_eq!(app.world().get::<TestComp>(entity), Some(&TestComp(5.0)));

        app.component::<TestComp>();
        app.interpolate_with_priority_filtered::<TestComp, With<SmoothRule>>(
            100,
            InterpolationFns::interpolate(marker_lerp),
        );
        *app.world_mut().get_mut::<TestComp>(entity).unwrap() = TestComp(0.0);

        app.update();

        assert_eq!(app.world().get::<TestComp>(entity), Some(&TestComp(42.0)));
    }

    #[test]
    fn bundle_interpolation_uses_tuple_interpolation_fn() {
        let mut app = App::new();
        app.add_plugins((TimePlugin, StatesPlugin, RepliconPlugins));
        app.insert_resource(ReplicationCheckpointMap::default());
        app.configure_sets(
            Update,
            (
                crate::plugin::InterpolationSystems::Cache,
                crate::plugin::InterpolationSystems::Prepare,
                crate::plugin::InterpolationSystems::Interpolate,
            )
                .chain(),
        );
        crate::plugin::add_update_interpolation_system(&mut app);
        app.component::<TestComp>().replicate();
        app.component::<TestComp2>().replicate();
        app.interpolate_bundle_with::<(TestComp, TestComp2)>(InterpolationFns::interpolate(
            bundle_lerp,
        ));

        let mut timeline = InterpolationTimeline::default();
        timeline.set_now(TickInstant::from(Tick(15)));
        timeline.remote_send_interval = core::time::Duration::from_millis(40);
        app.world_mut()
            .spawn((timeline, IsSynced::<InterpolationTimeline>::default()));

        let entity = app
            .world_mut()
            .spawn((
                Interpolated,
                TestComp(-1.0),
                TestComp2(-1.0),
                two_point_history(),
                two_point_history2(),
            ))
            .id();

        app.update();

        assert_eq!(app.world().get::<TestComp>(entity), Some(&TestComp(105.0)));
        assert_eq!(
            app.world().get::<TestComp2>(entity),
            Some(&TestComp2(205.0))
        );
    }

    #[test]
    fn bundle_interpolation_inserts_tuple_interpolated_components() {
        let mut app = App::new();
        app.add_plugins((TimePlugin, StatesPlugin, RepliconPlugins));
        app.insert_resource(ReplicationCheckpointMap::default());
        app.configure_sets(
            Update,
            (
                crate::plugin::InterpolationSystems::Cache,
                crate::plugin::InterpolationSystems::Prepare,
                crate::plugin::InterpolationSystems::Interpolate,
            )
                .chain(),
        );
        crate::plugin::add_update_interpolation_system(&mut app);
        app.component::<TestComp>().replicate();
        app.component::<TestComp2>().replicate();
        app.interpolate_bundle_with::<(TestComp, TestComp2)>(InterpolationFns::interpolate(
            bundle_lerp,
        ));

        let mut timeline = InterpolationTimeline::default();
        timeline.set_now(TickInstant::from(Tick(15)));
        timeline.remote_send_interval = core::time::Duration::from_millis(40);
        app.world_mut()
            .spawn((timeline, IsSynced::<InterpolationTimeline>::default()));

        let entity = app
            .world_mut()
            .spawn((Interpolated, two_point_history(), two_point_history2()))
            .id();

        app.update();

        assert_eq!(app.world().get::<TestComp>(entity), Some(&TestComp(105.0)));
        assert_eq!(
            app.world().get::<TestComp2>(entity),
            Some(&TestComp2(205.0))
        );
    }

    #[test]
    fn larger_default_bundle_priority_suppresses_smaller_overlapping_bundle() {
        BUNDLE2_PRIORITY_CALLS.store(0, Ordering::SeqCst);
        BUNDLE3_PRIORITY_CALLS.store(0, Ordering::SeqCst);

        let mut app = App::new();
        app.add_plugins((TimePlugin, StatesPlugin, RepliconPlugins));
        app.insert_resource(ReplicationCheckpointMap::default());
        app.configure_sets(
            Update,
            (
                crate::plugin::InterpolationSystems::Cache,
                crate::plugin::InterpolationSystems::Prepare,
                crate::plugin::InterpolationSystems::Interpolate,
            )
                .chain(),
        );
        crate::plugin::add_update_interpolation_system(&mut app);
        app.component::<TestComp>().replicate();
        app.component::<TestComp2>().replicate();
        app.component::<TestBundleComp<3>>().replicate();
        app.interpolate_bundle_with::<(TestComp, TestComp2)>(InterpolationFns::interpolate(
            bundle2_priority_lerp,
        ));
        app.interpolate_bundle_with::<(TestComp, TestComp2, TestBundleComp<3>)>(
            InterpolationFns::interpolate(bundle3_priority_lerp),
        );

        let mut timeline = InterpolationTimeline::default();
        timeline.set_now(TickInstant::from(Tick(15)));
        timeline.remote_send_interval = core::time::Duration::from_millis(40);
        app.world_mut()
            .spawn((timeline, IsSynced::<InterpolationTimeline>::default()));

        let entity = app
            .world_mut()
            .spawn((
                Interpolated,
                TestComp(-1.0),
                TestComp2(-1.0),
                TestBundleComp::<3>(-1.0),
                two_point_history(),
                two_point_history2(),
                two_point_bundle_history::<3>(),
            ))
            .id();

        app.update();

        assert_eq!(BUNDLE2_PRIORITY_CALLS.load(Ordering::SeqCst), 0);
        assert_eq!(BUNDLE3_PRIORITY_CALLS.load(Ordering::SeqCst), 1);
        assert_eq!(app.world().get::<TestComp>(entity), Some(&TestComp(305.0)));
        assert_eq!(
            app.world().get::<TestComp2>(entity),
            Some(&TestComp2(405.0))
        );
        assert_eq!(
            app.world().get::<TestBundleComp<3>>(entity),
            Some(&TestBundleComp::<3>(505.0))
        );
    }

    #[test]
    fn earlier_non_apply_member_rule_suppresses_same_priority_bundle_apply() {
        let mut app = App::new();
        app.add_plugins((TimePlugin, StatesPlugin, RepliconPlugins));
        app.insert_resource(ReplicationCheckpointMap::default());
        app.configure_sets(
            Update,
            (
                crate::plugin::InterpolationSystems::Cache,
                crate::plugin::InterpolationSystems::Prepare,
                crate::plugin::InterpolationSystems::Interpolate,
            )
                .chain(),
        );
        crate::plugin::add_update_interpolation_system(&mut app);
        app.component::<TestComp>().replicate();
        app.component::<TestComp2>().replicate();
        app.interpolate_with_priority::<TestComp>(2, InterpolationFns::history_only());
        app.interpolate_bundle_with::<(TestComp, TestComp2)>(InterpolationFns::interpolate(
            bundle_lerp,
        ));

        let mut timeline = InterpolationTimeline::default();
        timeline.set_now(TickInstant::from(Tick(15)));
        timeline.remote_send_interval = core::time::Duration::from_millis(40);
        app.world_mut()
            .spawn((timeline, IsSynced::<InterpolationTimeline>::default()));

        let entity = app
            .world_mut()
            .spawn((
                Interpolated,
                TestComp(-1.0),
                TestComp2(-1.0),
                two_point_history(),
                two_point_history2(),
            ))
            .id();

        app.update();

        assert_eq!(app.world().get::<TestComp>(entity), Some(&TestComp(-1.0)));
        assert_eq!(app.world().get::<TestComp2>(entity), Some(&TestComp2(-1.0)));
    }

    #[test]
    fn bundle_interpolation_supports_eight_component_tuple_api() {
        type Bundle8 = (
            TestBundleComp<1>,
            TestBundleComp<2>,
            TestBundleComp<3>,
            TestBundleComp<4>,
            TestBundleComp<5>,
            TestBundleComp<6>,
            TestBundleComp<7>,
            TestBundleComp<8>,
        );

        let mut app = App::new();
        app.add_plugins((TimePlugin, StatesPlugin, RepliconPlugins));
        app.insert_resource(ReplicationCheckpointMap::default());
        app.configure_sets(
            Update,
            (
                crate::plugin::InterpolationSystems::Cache,
                crate::plugin::InterpolationSystems::Prepare,
                crate::plugin::InterpolationSystems::Interpolate,
            )
                .chain(),
        );
        crate::plugin::add_update_interpolation_system(&mut app);
        app.component::<TestBundleComp<1>>().replicate();
        app.component::<TestBundleComp<2>>().replicate();
        app.component::<TestBundleComp<3>>().replicate();
        app.component::<TestBundleComp<4>>().replicate();
        app.component::<TestBundleComp<5>>().replicate();
        app.component::<TestBundleComp<6>>().replicate();
        app.component::<TestBundleComp<7>>().replicate();
        app.component::<TestBundleComp<8>>().replicate();
        app.interpolate_bundle_with::<Bundle8>(InterpolationFns::interpolate(bundle8_lerp));

        let mut timeline = InterpolationTimeline::default();
        timeline.set_now(TickInstant::from(Tick(15)));
        timeline.remote_send_interval = core::time::Duration::from_millis(40);
        app.world_mut()
            .spawn((timeline, IsSynced::<InterpolationTimeline>::default()));

        let entity = app
            .world_mut()
            .spawn((
                Interpolated,
                TestBundleComp::<1>(-1.0),
                TestBundleComp::<2>(-1.0),
                TestBundleComp::<3>(-1.0),
                TestBundleComp::<4>(-1.0),
                TestBundleComp::<5>(-1.0),
                TestBundleComp::<6>(-1.0),
                TestBundleComp::<7>(-1.0),
                TestBundleComp::<8>(-1.0),
            ))
            .insert((
                two_point_bundle_history::<1>(),
                two_point_bundle_history::<2>(),
                two_point_bundle_history::<3>(),
                two_point_bundle_history::<4>(),
                two_point_bundle_history::<5>(),
                two_point_bundle_history::<6>(),
                two_point_bundle_history::<7>(),
                two_point_bundle_history::<8>(),
            ))
            .id();

        app.update();

        assert_eq!(
            app.world().get::<TestBundleComp<1>>(entity),
            Some(&TestBundleComp::<1>(15.0))
        );
        assert_eq!(
            app.world().get::<TestBundleComp<2>>(entity),
            Some(&TestBundleComp::<2>(25.0))
        );
        assert_eq!(
            app.world().get::<TestBundleComp<3>>(entity),
            Some(&TestBundleComp::<3>(35.0))
        );
        assert_eq!(
            app.world().get::<TestBundleComp<4>>(entity),
            Some(&TestBundleComp::<4>(45.0))
        );
        assert_eq!(
            app.world().get::<TestBundleComp<5>>(entity),
            Some(&TestBundleComp::<5>(55.0))
        );
        assert_eq!(
            app.world().get::<TestBundleComp<6>>(entity),
            Some(&TestBundleComp::<6>(65.0))
        );
        assert_eq!(
            app.world().get::<TestBundleComp<7>>(entity),
            Some(&TestBundleComp::<7>(75.0))
        );
        assert_eq!(
            app.world().get::<TestBundleComp<8>>(entity),
            Some(&TestBundleComp::<8>(85.0))
        );
    }

    #[test]
    fn update_confirmed_history_advances_to_latest_empty_mutate_tick_when_idle() {
        let mut app = setup_app(Tick(30), 40);
        crate::plugin::add_update_interpolation_system(&mut app);
        confirm_server_tick(&mut app, 1, Tick(30));

        let entity = app.world_mut().spawn(TestComp(9.5)).id();
        let mut history = ConfirmedHistory::<TestComp>::default();
        history.insert_present(Tick(10), TestComp(0.0));
        history.insert_present(Tick(20), TestComp(10.0));
        insert_confirmed_history(&mut app, entity, history);

        app.update();

        let component = app.world().get::<TestComp>(entity).unwrap();
        let history = app
            .world()
            .get::<ConfirmedHistory<TestComp>>(entity)
            .unwrap();
        assert_eq!(component, &TestComp(10.0));
        assert_eq!(history.len(), 2);
        assert_eq!(
            history.start_present().map(|(t, v)| (t, v.clone())),
            Some((Tick(20), TestComp(10.0)))
        );
        assert_eq!(
            history.get_nth_present(1).map(|(t, v)| (t, v.clone())),
            Some((Tick(30), TestComp(10.0)))
        );
    }

    #[test]
    fn update_confirmed_history_records_repeated_empty_mutate_ticks() {
        let mut app = setup_app(Tick(25), 40);
        crate::plugin::add_update_interpolation_system(&mut app);
        confirm_server_tick(&mut app, 1, Tick(30));

        let entity = app.world_mut().spawn(TestComp(9.5)).id();
        let mut history = ConfirmedHistory::<TestComp>::default();
        history.insert_present(Tick(20), TestComp(10.0));
        insert_confirmed_history(&mut app, entity, history);

        app.update();
        confirm_server_tick(&mut app, 2, Tick(31));
        app.update();

        let history = app
            .world()
            .get::<ConfirmedHistory<TestComp>>(entity)
            .unwrap();
        assert_eq!(history.len(), 3);
        assert_eq!(
            history.start_present().map(|(t, v)| (t, v.clone())),
            Some((Tick(20), TestComp(10.0)))
        );
        assert_eq!(
            history.get_nth_present(1).map(|(t, v)| (t, v.clone())),
            Some((Tick(30), TestComp(10.0)))
        );
        assert_eq!(
            history.get_nth_present(2).map(|(t, v)| (t, v.clone())),
            Some((Tick(31), TestComp(10.0)))
        );
    }

    #[test]
    fn diff_history_waits_when_completed_tick_diff_is_pending() {
        let mut app = setup_app(Tick(5), 40);
        use_diff_history_rule(&mut app);
        crate::plugin::add_update_interpolation_system(&mut app);
        confirm_server_tick(&mut app, 1, Tick(5));

        let mut history = ConfirmedHistory::<TestComp>::default();
        history.insert_present(Tick(0), TestComp(0.0));
        let mut receiver = HistoryDiffReceiver::<TestComp>::default();
        receiver.record_cursor(Tick(0), Some(idx(0)));
        receiver
            .queue_diffs(Tick(5), idx(4), vec![4.0, 5.0])
            .unwrap();

        let entity = app.world_mut().spawn((Interpolated, history)).id();
        app.world_mut()
            .resource_mut::<ReplicationStorage>()
            .insert(entity, receiver);

        app.update();

        let history = app
            .world()
            .get::<ConfirmedHistory<TestComp>>(entity)
            .unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(
            history.start_present().map(|(t, v)| (t, v.clone())),
            Some((Tick(0), TestComp(0.0)))
        );

        let receiver = app
            .world()
            .resource::<ReplicationStorage>()
            .get::<HistoryDiffReceiver<TestComp>>(entity)
            .unwrap();
        assert!(receiver.has_pending_diffs());
        assert_eq!(receiver.tick_for_cursor(Some(idx(0))), Some(Tick(0)));
    }

    #[test]
    fn diff_history_without_receiver_does_not_remove_live_component() {
        let mut app = setup_app(Tick(5), 40);
        use_diff_history_rule(&mut app);
        crate::plugin::add_update_interpolation_system(&mut app);

        let entity = app
            .world_mut()
            .spawn((
                Interpolated,
                TestComp(12.0),
                ConfirmedHistory::<TestComp>::default(),
            ))
            .id();

        app.update();

        assert_eq!(app.world().get::<TestComp>(entity), Some(&TestComp(12.0)));
    }

    #[test]
    fn update_confirmed_history_diff_advances_when_only_older_diff_is_pending() {
        let mut app = setup_app(Tick(6), 40);
        use_diff_history_rule(&mut app);
        crate::plugin::add_update_interpolation_system(&mut app);
        confirm_server_tick(&mut app, 1, Tick(6));

        let mut history = ConfirmedHistory::<TestComp>::default();
        history.insert_present(Tick(0), TestComp(0.0));
        let mut receiver = HistoryDiffReceiver::<TestComp>::default();
        receiver.record_cursor(Tick(0), Some(idx(0)));
        receiver
            .queue_diffs(Tick(5), idx(4), vec![4.0, 5.0])
            .unwrap();

        let entity = app.world_mut().spawn((Interpolated, history)).id();
        app.world_mut()
            .resource_mut::<ReplicationStorage>()
            .insert(entity, receiver);

        app.update();

        let history = app
            .world()
            .get::<ConfirmedHistory<TestComp>>(entity)
            .unwrap();
        assert_eq!(history.len(), 2);
        assert_eq!(
            history.start_present().map(|(t, v)| (t, v.clone())),
            Some((Tick(0), TestComp(0.0)))
        );
        assert_eq!(
            history.get_nth_present(1).map(|(t, v)| (t, v.clone())),
            Some((Tick(6), TestComp(0.0)))
        );

        let receiver = app
            .world()
            .resource::<ReplicationStorage>()
            .get::<HistoryDiffReceiver<TestComp>>(entity)
            .unwrap();
        assert!(receiver.has_pending_diffs());
        assert_eq!(receiver.tick_for_cursor(Some(idx(0))), Some(Tick(0)));
    }

    #[test]
    fn update_confirmed_history_does_not_move_history_backwards() {
        let mut app = setup_app(Tick(30), 40);
        crate::plugin::add_update_interpolation_system(&mut app);
        confirm_server_tick(&mut app, 1, Tick(100));

        let entity = app.world_mut().spawn(TestComp(9.5)).id();
        let mut history = ConfirmedHistory::<TestComp>::default();
        history.insert_present(Tick(120), TestComp(10.0));
        insert_confirmed_history(&mut app, entity, history);

        app.update();

        let history = app
            .world()
            .get::<ConfirmedHistory<TestComp>>(entity)
            .unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(
            history.start_present().map(|(t, v)| (t, v.clone())),
            Some((Tick(120), TestComp(10.0)))
        );
    }

    #[test]
    fn update_confirmed_history_advances_from_server_mutate_ticks_without_entity_confirm_history() {
        let mut app = setup_app(Tick(30), 40);
        crate::plugin::add_update_interpolation_system(&mut app);
        confirm_server_tick(&mut app, 1, Tick(20));
        confirm_server_tick(&mut app, 2, Tick(30));

        let entity = app.world_mut().spawn(TestComp(9.5)).id();
        let mut history = ConfirmedHistory::<TestComp>::default();
        history.insert_present(Tick(10), TestComp(10.0));
        insert_confirmed_history(&mut app, entity, history);

        app.update();

        let history = app
            .world()
            .get::<ConfirmedHistory<TestComp>>(entity)
            .unwrap();
        assert_eq!(history.len(), 2);
        assert_eq!(
            history.get_nth_present(1).map(|(t, v)| (t, v.clone())),
            Some((Tick(30), TestComp(10.0)))
        );
    }

    #[test]
    fn update_confirmed_history_keeps_bracketing_pair_during_loss_gap() {
        let mut app = setup_app(Tick(25), 40);
        crate::plugin::add_update_interpolation_system(&mut app);

        let entity = app.world_mut().spawn(TestComp(999.0)).id();
        let mut history = ConfirmedHistory::<TestComp>::default();
        history.insert_present(Tick(10), TestComp(0.0));
        history.insert_present(Tick(20), TestComp(10.0));
        history.insert_present(Tick(30), TestComp(20.0));
        insert_confirmed_history(&mut app, entity, history);

        app.update();

        let component = app.world().get::<TestComp>(entity).unwrap();
        let history = app
            .world()
            .get::<ConfirmedHistory<TestComp>>(entity)
            .unwrap();
        assert_eq!(history.len(), 2);
        assert_eq!(
            history.start_present().map(|(t, v)| (t, v.clone())),
            Some((Tick(20), TestComp(10.0)))
        );
        assert_eq!(
            history.get_nth_present(1).map(|(t, v)| (t, v.clone())),
            Some((Tick(30), TestComp(20.0)))
        );
        assert_eq!(component, &TestComp(15.0));
    }

    #[test]
    fn update_confirmed_history_waits_to_insert_component_until_start_tick() {
        let mut app = setup_app(Tick(9), 40);
        crate::plugin::add_update_interpolation_system(&mut app);

        let entity = app.world_mut().spawn_empty().id();
        let mut history = ConfirmedHistory::<TestComp>::default();
        history.insert_present(Tick(10), TestComp(0.0));
        history.insert_present(Tick(20), TestComp(10.0));
        insert_confirmed_history(&mut app, entity, history);

        app.update();

        assert!(!app.world().entity(entity).contains::<TestComp>());
    }

    #[test]
    fn update_confirmed_history_removes_component_until_start_tick() {
        let mut app = setup_app(Tick(9), 40);
        crate::plugin::add_update_interpolation_system(&mut app);

        let entity = app.world_mut().spawn(TestComp(99.0)).id();
        let mut history = ConfirmedHistory::<TestComp>::default();
        history.insert_present(Tick(10), TestComp(0.0));
        history.insert_present(Tick(20), TestComp(10.0));
        insert_confirmed_history(&mut app, entity, history);

        app.update();

        assert!(!app.world().entity(entity).contains::<TestComp>());
    }

    #[test]
    fn update_confirmed_history_inserts_and_interpolates_when_start_tick_is_reached() {
        let mut app = setup_app(Tick(15), 40);
        crate::plugin::add_update_interpolation_system(&mut app);

        let entity = app.world_mut().spawn_empty().id();
        let mut history = ConfirmedHistory::<TestComp>::default();
        history.insert_present(Tick(10), TestComp(0.0));
        history.insert_present(Tick(20), TestComp(10.0));
        insert_confirmed_history(&mut app, entity, history);

        app.update();

        assert_eq!(app.world().get::<TestComp>(entity), Some(&TestComp(5.0)));
    }

    #[test]
    fn component_removal_waits_until_interpolation_tick_reaches_remove_tick() {
        let mut app = setup_app(Tick(15), 40);
        crate::plugin::add_update_interpolation_system(&mut app);

        let entity = app.world_mut().spawn(TestComp(99.0)).id();
        let mut history = ConfirmedHistory::<TestComp>::default();
        history.insert_present(Tick(10), TestComp(10.0));
        history.insert_removed(Tick(20));
        insert_confirmed_history(&mut app, entity, history);

        app.update();
        assert_eq!(app.world().get::<TestComp>(entity), Some(&TestComp(10.0)));

        set_interpolation_tick(&mut app, Tick(20));
        app.update();
        assert!(!app.world().entity(entity).contains::<TestComp>());
    }

    #[test]
    fn component_reinsert_after_removal_waits_until_insert_tick() {
        let mut app = setup_app(Tick(15), 40);
        crate::plugin::add_update_interpolation_system(&mut app);

        let entity = app.world_mut().spawn_empty().id();
        let mut history = ConfirmedHistory::<TestComp>::default();
        history.insert_removed(Tick(10));
        history.insert_present(Tick(20), TestComp(20.0));
        insert_confirmed_history(&mut app, entity, history);

        app.update();
        assert!(!app.world().entity(entity).contains::<TestComp>());

        set_interpolation_tick(&mut app, Tick(20));
        app.update();
        assert_eq!(app.world().get::<TestComp>(entity), Some(&TestComp(20.0)));
    }

    #[test]
    fn update_confirmed_history_seeds_component_at_current_interpolation_sample() {
        let mut app = setup_app(Tick(15), 40);
        crate::plugin::add_update_interpolation_system(&mut app);

        let entity = app.world_mut().spawn_empty().id();
        let mut history = ConfirmedHistory::<TestComp>::default();
        history.insert_present(Tick(10), TestComp(0.0));
        history.insert_present(Tick(20), TestComp(10.0));
        insert_confirmed_history(&mut app, entity, history);

        app.update();

        assert_eq!(app.world().get::<TestComp>(entity), Some(&TestComp(5.0)));
    }

    #[test]
    fn stale_entity_insert_command_after_despawn_does_not_panic() {
        let mut app = App::new();
        let entity = app.world_mut().spawn_empty().id();
        app.add_systems(Update, move |mut commands: Commands| {
            commands.entity(entity).despawn();
            commands.entity(entity).try_insert(TestComp(1.0));
        });

        app.update();

        assert!(app.world().get_entity(entity).is_err());
    }
}
//
//     use std::net::SocketAddr;
//     use std::str::FromStr;
//     use bevy::utils::{Duration, Instant};
//
//     use bevy::log::LogPlugin;
//     use bevy::prelude::*;
//     use bevy::time::TimeUpdateStrategy;
//     use bevy::{DefaultPlugins, MinimalPlugins};
//     use tracing::{debug, info};
//     use tracing_subscriber::fmt::format::FmtSpan;
//
//     use crate::_reexport::*;
//     use crate::prelude::client::*;
//     use crate::prelude::*;
//     use crate::tests::protocol::*;
//     use crate::tests::stepper::{BevyStepper};
//
//     fn setup() -> (BevyStepper, Entity, Entity) {
//         let frame_duration = Duration::from_millis(10);
//         let tick_duration = Duration::from_millis(10);
//         let shared_config = SharedConfig {
//             enable_replication: false,
//             tick: TickConfig::new(tick_duration),
//             ..Default::default()
//         };
//         let link_conditioner = LinkConditionerConfig {
//             incoming_latency: Duration::from_millis(40),
//             incoming_jitter: Duration::from_millis(5),
//             incoming_loss: 0.05,
//         };
//         let sync_config = SyncConfig::default().speedup_factor(1.0);
//         let prediction_config = PredictionConfig::default().disable(true);
//         let interpolation_delay = Duration::from_millis(100);
//         let interpolation_config = InterpolationConfig::default().with_delay(InterpolationDelay {
//             min_delay: interpolation_delay,
//             send_interval_ratio: 0.0,
//         });
//         let mut stepper = BevyStepper::new(
//             shared_config,
//             sync_config,
//             prediction_config,
//             interpolation_config,
//             link_conditioner,
//             frame_duration,
//         );
//         stepper.init();
//
//         // Create a confirmed entity on the server
//         let server_entity = stepper
//             .server_app
//             .world_mut()
//             .spawn((Component1(0.0), ShouldBeInterpolated))
//             .id();
//
//         // Set the latest received server tick
//         let confirmed_tick = stepper.client_app().world_mut().resource_mut::<ClientConnectionManager>()
//             .replication_receiver
//             .remote_entity_map
//             .get_confirmed_tick(confirmed_entity)
//             .unwrap();
//
//         // Tick once
//         stepper.frame_step();
//         let tick = stepper.client_tick();
//         let interpolated = stepper
//             .client_app
//             .world()//             .get::<Confirmed>(confirmed)
//             .unwrap()
//             .interpolated
//             .unwrap();
//
//         assert_eq!(
//             stepper
//                 .client_app
//                 .world()//                 .get::<Component1>(confirmed)
//                 .unwrap(),
//             &Component1(0.0)
//         );
//
//         // check that the interpolated entity got spawned
//         assert_eq!(
//             stepper
//                 .client_app
//                 .world()//                 .get::<Interpolated>(interpolated)
//                 .unwrap()
//                 .confirmed_entity,
//             confirmed
//         );
//
//         // check that the component history got created and is empty
//         let history = ConfirmedHistory::<Component1>::new();
//         assert_eq!(
//             stepper
//                 .client_app
//                 .world()//                 .get::<ConfirmedHistory<Component1>>(interpolated)
//                 .unwrap(),
//             &history,
//         );
//         // check that the confirmed component got replicated
//         assert_eq!(
//             stepper
//                 .client_app
//                 .world()//                 .get::<Component1>(interpolated)
//                 .unwrap(),
//             &Component1(0.0)
//         );
//         // check that the interpolate status got updated
//         let interpolation_tick = stepper.interpolation_tick();
//         assert_eq!(
//             stepper
//                 .client_app
//                 .world()//                 .get::<InterpolateStatus<Component1>>(interpolated)
//                 .unwrap(),
//             &InterpolateStatus::<Component1> {
//                 start: None,
//                 end: (tick, Component1(0.0)).into(),
//                 current: interpolation_tick,
//             }
//         );
//         (stepper, confirmed, interpolated)
//     }
//
//     // Test interpolation
//     #[test]
//     fn test_interpolation() -> anyhow::Result<()> {
//         let (mut stepper, confirmed, interpolated) = setup();
//         let start_tick = stepper.client_tick();
//         // reach interpolation start tick
//         stepper.frame_step();
//         stepper.frame_step();
//
//         // check that the interpolate status got updated (end becomes start)
//         assert_eq!(
//             stepper
//                 .client_app
//                 .world()//                 .get::<InterpolateStatus<Component1>>(interpolated)
//                 .unwrap(),
//             &InterpolateStatus::<Component1> {
//                 start: (Tick(0), Component1(0.0)).into(),
//                 end: None,
//                 current: Tick(3),
//                 // current: Tick(3) - interpolation_tick_delay,
//             }
//         );
//
//         // receive server update
//         // stepper
//         //     .client_mut()
//         //     .set_latest_received_server_tick(Tick(2));
//         stepper
//             .client_app
//             .world_mut()
//             .get_entity_mut(confirmed)
//             .unwrap()
//             .get_mut::<Component1>()
//             .unwrap()
//             .0 = 2.0;
//
//         stepper.frame_step();
//         // check that interpolation is working correctly
//         assert_eq!(
//             stepper
//                 .client_app
//                 .world()//                 .get::<InterpolateStatus<Component1>>(interpolated)
//                 .unwrap(),
//             &InterpolateStatus::<Component1> {
//                 start: (Tick(0), Component1(0.0)).into(),
//                 end: (Tick(2), Component1(2.0)).into(),
//                 current: Tick(4),
//                 // current: Tick(4) - interpolation_tick_delay,
//             }
//         );
//         assert_eq!(
//             stepper
//                 .client_app
//                 .world()//                 .get::<Component1>(interpolated)
//                 .unwrap(),
//             &Component1(1.0)
//         );
//         stepper.frame_step();
//         assert_eq!(
//             stepper
//                 .client_app
//                 .world()//                 .get::<InterpolateStatus<Component1>>(interpolated)
//                 .unwrap(),
//             &InterpolateStatus::<Component1> {
//                 start: (Tick(2), Component1(2.0)).into(),
//                 end: None,
//                 current: Tick(5),
//                 // current: Tick(5) - interpolation_tick_delay,
//             }
//         );
//         assert_eq!(
//             stepper
//                 .client_app
//                 .world()//                 .get::<Component1>(interpolated)
//                 .unwrap(),
//             &Component1(2.0)
//         );
//         Ok(())
//     }
//
//     // We are in the situation: S1 < I
//     // where S1 is a confirmed ticks, and I is the interpolated tick
//     // and we receive S1 < S2 < I
//     // Then we should now start interpolating from S2
//     #[test]
//     fn test_received_more_recent_start() -> anyhow::Result<()> {
//         let (mut stepper, confirmed, interpolated) = setup();
//
//         // reach interpolation start tick
//         stepper.frame_step();
//         stepper.frame_step();
//         stepper.frame_step();
//         stepper.frame_step();
//         assert_eq!(stepper.client_tick(), Tick(5));
//
//         // receive server update
//         // stepper
//         //     .client_mut()
//         //     .set_latest_received_server_tick(Tick(1));
//         stepper
//             .client_app
//             .world_mut()
//             .get_entity_mut(confirmed)
//             .unwrap()
//             .get_mut::<Component1>()
//             .unwrap()
//             .0 = 1.0;
//
//         stepper.frame_step();
//         // check the status uses the more recent server update
//         assert_eq!(
//             stepper
//                 .client_app
//                 .world()//                 .get::<InterpolateStatus<Component1>>(interpolated)
//                 .unwrap(),
//             &InterpolateStatus::<Component1> {
//                 start: (Tick(1), Component1(1.0)).into(),
//                 end: None,
//                 current: Tick(6),
//                 // current: Tick(6) - interpolation_tick_delay,
//             }
//         );
//         Ok(())
//     }
// }
