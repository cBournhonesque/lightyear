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
use bevy_ecs::world::unsafe_world_cell::UnsafeWorldCell;
use bevy_replicon::shared::replication::diff::Diffable as RepliconDiffable;
use bevy_replicon::shared::replication::storage::ReplicationStorage;
use bevy_utils::prelude::DebugName;
use lightyear_core::ecs_utils::{
    table_component_slice, table_for_archetype, write_component_with_change_detection,
};
use lightyear_core::history_buffer::HistoryState;
use lightyear_core::prelude::{ConfirmedHistory, NetworkTimeline};
use lightyear_core::tick::Tick;
use lightyear_core::tick::TickDuration;
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
/// stores the independently resolved history and apply owners for each
/// archetype and the type-erased history functions that should run there.
///
/// Component insertion/removal happens here so custom interpolation systems that
/// run after [`crate::plugin::InterpolationSystems::Prepare`] see the live
/// component set matching the interpolation timeline.
pub(crate) fn update_interpolation_history(
    mut interpolation_world: InterpolationWorld,
    timeline: Res<InterpolationTimeline>,
    interpolation_registry: Res<InterpolationRegistry>,
    checkpoints: Res<ReplicationCheckpointMap>,
    mut replication_storage: Option<ResMut<ReplicationStorage>>,
    mut commands: Commands,
) {
    // TODO: exclude host-server
    let current_interpolate_tick = timeline.now().tick();
    let server_complete_tick = checkpoints.last_confirmed_tick();
    let ctx = UpdateHistoryContext {
        server_complete_tick,
        current_interpolate_tick,
    };

    let mut deferred_apply = DeferredEntityCommands::default();

    interpolation_world.update_archetypes(&interpolation_registry);
    let world = interpolation_world.world;
    for (archetype, cached_archetype) in interpolation_world.iter_archetypes() {
        for component in &cached_archetype.history_components {
            if !component.history_component_present {
                for entity in archetype.entities() {
                    (component.insert_history)(entity.id(), &mut commands);
                }
                continue;
            }
            (component.update_history)(
                world,
                archetype,
                component,
                &ctx,
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
    timeline: Res<InterpolationTimeline>,
    interpolation_registry: Res<InterpolationRegistry>,
    tick_duration: Option<Res<TickDuration>>,
) {
    // TODO: exclude host-server
    let current_interpolate_tick = timeline.now().tick();
    let interpolation_overstep = timeline.overstep().to_f32();
    let ctx = ApplyInterpolationContext {
        interpolation_tick: current_interpolate_tick,
        interpolation_overstep,
        tick_duration: tick_duration.as_deref().map(|duration| duration.0),
    };

    interpolation_world.update_archetypes(&interpolation_registry);
    let world = interpolation_world.world;
    for (archetype, cached_archetype) in interpolation_world.iter_archetypes() {
        for component in &cached_archetype.apply_callbacks {
            (component.apply_interpolation)(
                world,
                archetype,
                &interpolation_registry,
                component.rule_id,
                ctx,
            );
        }
    }
}

/// Maintains existing `ConfirmedHistory<C>` values for one cached archetype.
///
/// It records completed server ticks as unchanged when appropriate, prunes old
/// entries, and queues live `C` insertion or removal when the interpolation
/// timeline crosses a presence boundary. Missing histories are initialized by
/// the rule's separate history-insertion callback before this function runs.
pub(crate) fn update_history_archetype_erased<C: Component + Clone>(
    world: UnsafeWorldCell,
    archetype: &Archetype,
    component: &CachedInterpolationComponent,
    ctx: &UpdateHistoryContext,
    _replication_storage: Option<&mut ReplicationStorage>,
    deferred_apply: &mut DeferredEntityCommands,
) {
    let Some(StorageType::Table) = component.history_storage else {
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
        table_component_slice::<ConfirmedHistory<C>>(table, component.history_component_id)
    else {
        return;
    };
    let present = component.live_component_present;
    for entity in archetype.entities() {
        let entity_id = entity.id();
        let row = entity.table_row().index();
        let history = unsafe { &mut *histories.get_unchecked(row).get() };
        update_history_inner::<C>(history, entity_id, ctx);
        let state = history.get_state_at_or_before(ctx.current_interpolate_tick);
        queue_history_presence::<C>(deferred_apply, entity_id, present, state);
    }
}

/// Maintains diff-backed `ConfirmedHistory<C>` values for one cached archetype.
///
/// Unlike [`update_history_archetype_erased`], this waits for pending diffs in
/// `ReplicationStorage` before advancing unchanged state or pruning history.
/// It also queues live `C` insertion or removal at interpolation-time presence
/// boundaries.
pub(crate) fn update_history_diff_archetype_erased<C>(
    world: UnsafeWorldCell,
    archetype: &Archetype,
    component: &CachedInterpolationComponent,
    ctx: &UpdateHistoryContext,
    replication_storage: Option<&mut ReplicationStorage>,
    deferred_apply: &mut DeferredEntityCommands,
) where
    C: Component + Clone + RepliconDiffable,
{
    let Some(StorageType::Table) = component.history_storage else {
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
        table_component_slice::<ConfirmedHistory<C>>(table, component.history_component_id)
    else {
        return;
    };
    let Some(storage) = replication_storage else {
        return;
    };
    let present = component.live_component_present;
    for entity in archetype.entities() {
        let entity_id = entity.id();
        let Some(history_diff_receiver) = storage.get_mut::<HistoryDiffReceiver<C>>(entity_id)
        else {
            continue;
        };
        let row = entity.table_row().index();
        let history = unsafe { &mut *histories.get_unchecked(row).get() };

        // At a completed checkpoint C, a diff component has exactly one of three outcomes:
        // 1. The component changed at C and the diff was materialized, so the write callback
        //    already inserted its authoritative state at C. Check ConfirmedHistory first and avoid
        //    consulting pending diff storage in this case.
        // 2. An update at or before C is still pending, so its concrete state at C is unresolved
        //    and ConfirmedHistory must not receive an entry at C yet.
        // 3. All diffs through C are materialized and there was no update at C, so inserting
        //    SameAsPrecedent records the final authoritative value carried forward to C.
        if let Some(server_complete_tick) = ctx.server_complete_tick
            && history.get_state_at(server_complete_tick).is_none()
            && !history_diff_receiver.has_pending_diff_at_or_before(server_complete_tick)
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
        }

        let state = history.get_state_at_or_before(ctx.current_interpolate_tick);
        queue_history_presence::<C>(deferred_apply, entity_id, present, state);
    }
}

fn update_history_inner<C: Component + Clone>(
    history: &mut ConfirmedHistory<C>,
    entity: Entity,
    ctx: &UpdateHistoryContext,
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
    state: Option<&HistoryState<C>>,
) {
    // Apply the history state for the current interpolation time to the live component set:
    // insert once the add/update tick becomes visible, remove once a removal tick is reached,
    // and otherwise leave the current component value alone.
    match state {
        None | Some(HistoryState::Removed) if present => {
            deferred_apply.remove::<C>(entity);
        }
        Some(HistoryState::Updated(value)) if !present => {
            deferred_apply.insert(entity, value.clone());
        }
        _ => {}
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
    let Some(histories) = table_component_slice::<ConfirmedHistory<C>>(table, history_component_id)
    else {
        return;
    };
    let interpolation = interpolation_registry.interpolation_fn_for_rule::<C>(rule_id);

    for entity in archetype.entities() {
        let row = entity.table_row().index();
        let history = unsafe { &*histories.get_unchecked(row).get() };
        let Some(HistoryState::Updated(interpolated)) = sample_history_with_interpolation(
            interpolation,
            history,
            ctx.interpolation_tick,
            ctx.interpolation_overstep,
            ctx.tick_duration,
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
        // SAFETY: the erased interpolation system declares write access to C,
        // and no reference to this entity's live C is held here.
        unsafe {
            write_component_with_change_detection::<C>(world, entity.id(), interpolated);
        }
    }
}

/// Resolved history entry at or before a tick and its successor.
pub(crate) struct HistoryBracket<'a, C> {
    pub(crate) start_tick: Tick,
    pub(crate) start_state: &'a HistoryState<C>,
    pub(crate) end: Option<(Tick, &'a HistoryState<C>)>,
}

/// Returns the immediate resolved history window around `tick`.
///
/// Both component and bundle interpolation use this lookup so they agree on
/// the bracketing entries around the interpolation timeline.
pub(crate) fn history_bracket<C>(
    history: &ConfirmedHistory<C>,
    tick: Tick,
) -> Option<HistoryBracket<'_, C>> {
    let previous_index = (0..history.len())
        .take_while(|i| {
            history
                .get_nth_tick(*i)
                .is_some_and(|history_tick| history_tick <= tick)
        })
        .last()?;

    let (start_tick, start_state) = history.get_nth_state(previous_index)?;
    Some(HistoryBracket {
        start_tick,
        start_state,
        end: history.get_nth_state(previous_index + 1),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin::InterpolationMarkerPlugin;
    use crate::registry::{
        AppInterpolationExt, InterpolationRegistry, component_rule,
        insert_confirmed_history as insert_confirmed_history_component,
        insert_confirmed_history_diff,
    };
    use crate::rules::{
        InterpolationFns, InterpolationFnsExt, InterpolationRuleConfig, InterpolationSampleContext,
    };
    use alloc::vec;
    use bevy_app::{App, Update};
    use bevy_ecs::archetype::Archetype;
    use bevy_ecs::component::Component;
    use bevy_ecs::query::{ArchetypeFilter, QueryState};
    use bevy_ecs::schedule::IntoScheduleConfigs;
    use bevy_math::{
        Curve,
        curve::{Ease, FunctionCurve, Interval},
    };
    use bevy_replicon::prelude::{
        Diffable as RepliconDiffable, RepliconPlugins, RepliconSharedPlugin, RepliconTick,
    };
    use bevy_replicon::shared::replication::diff::diff_index::DiffIndex;
    use bevy_state::app::StatesPlugin;
    use bevy_time::TimePlugin;
    use core::sync::atomic::{AtomicUsize, Ordering};
    use lightyear_core::prelude::Interpolated;
    use lightyear_core::tick::TickDuration;
    use lightyear_core::time::TickInstant;
    use lightyear_replication::checkpoint::ReplicationCheckpointMap;
    use lightyear_replication::diff_history::HistoryDiffReceiver;
    use lightyear_replication::registry::replication::AppComponentExt;
    use serde::{Deserialize, Serialize};

    #[derive(Component, Clone, Debug, Deserialize, PartialEq, Serialize)]
    struct TestComp(f32);

    impl Ease for TestComp {
        fn interpolating_curve_unbounded(start: Self, end: Self) -> impl Curve<Self> {
            FunctionCurve::new(Interval::UNIT, move |t| {
                TestComp(start.0 + (end.0 - start.0) * t)
            })
        }
    }

    #[derive(Component, Clone, Debug, Deserialize, PartialEq, Serialize)]
    struct TestComp2(f32);

    #[derive(Component, Clone, Debug, Deserialize, PartialEq, Serialize)]
    struct TestBundleComp<const N: usize>(f32);

    #[derive(Component)]
    struct SmoothRule;

    #[derive(Component)]
    struct HistoryOnlyRule;

    #[derive(Component)]
    struct NoHistoryRule;

    #[derive(Component)]
    struct DisabledRule;

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

    fn lerp2(start: TestComp2, end: TestComp2, t: f32) -> TestComp2 {
        TestComp2(start.0 + (end.0 - start.0) * t)
    }

    fn context_lerp(_start: TestComp, _end: TestComp, ctx: InterpolationSampleContext) -> TestComp {
        TestComp(ctx.t + ctx.sample_delta_secs.unwrap_or_default())
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

    fn bundle_context_lerp(
        _start: (TestComp, TestComp2),
        _end: (TestComp, TestComp2),
        ctx: InterpolationSampleContext,
    ) -> (TestComp, TestComp2) {
        (
            TestComp(ctx.t),
            TestComp2(ctx.sample_delta_secs.unwrap_or_default()),
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
        app.add_plugins((
            StatesPlugin,
            RepliconSharedPlugin::default(),
            InterpolationMarkerPlugin,
        ));
        app.world_mut()
            .insert_resource(ReplicationCheckpointMap::default());
        app.world_mut()
            .insert_resource(ReplicationStorage::default());
        let mut registry = InterpolationRegistry::default();
        let fns = InterpolationFns::interpolate(lerp);
        let rule = component_rule::<TestComp, ()>(
            app.world_mut(),
            fns,
            InterpolationRuleConfig::default(),
            update_history_archetype_erased::<TestComp>,
            insert_confirmed_history_component::<TestComp>,
        );
        registry.insert_rule(rule);
        app.world_mut().insert_resource(registry);

        let mut timeline = InterpolationTimeline::default();
        timeline.set_now(TickInstant::from(current_tick));
        timeline.remote_send_interval = core::time::Duration::from_millis(send_interval_ms);
        app.insert_resource(timeline);
        app
    }

    struct InterpolationTestAppBuilder {
        interpolation_tick: Tick,
        tick_duration: Option<core::time::Duration>,
    }

    impl InterpolationTestAppBuilder {
        fn new(interpolation_tick: Tick) -> Self {
            Self {
                interpolation_tick,
                tick_duration: None,
            }
        }

        fn tick_duration(mut self, tick_duration: core::time::Duration) -> Self {
            self.tick_duration = Some(tick_duration);
            self
        }

        fn build(self) -> App {
            let mut app = App::new();
            app.add_plugins((
                TimePlugin,
                StatesPlugin,
                RepliconPlugins,
                InterpolationMarkerPlugin,
            ));
            app.insert_resource(ReplicationCheckpointMap::default());
            app.configure_sets(
                Update,
                (
                    crate::plugin::InterpolationSystems::Prepare,
                    crate::plugin::InterpolationSystems::Interpolate,
                )
                    .chain(),
            );
            add_interpolation_test_systems(&mut app);

            if let Some(tick_duration) = self.tick_duration {
                app.insert_resource(TickDuration(tick_duration));
            }

            let mut timeline = InterpolationTimeline::default();
            timeline.set_now(TickInstant::from(self.interpolation_tick));
            timeline.remote_send_interval = core::time::Duration::from_millis(40);
            app.insert_resource(timeline);
            app
        }
    }

    fn confirm_server_tick(app: &mut App, replicon_tick: u32, server_tick: Tick) {
        let replicon_tick = RepliconTick::new(replicon_tick);
        let mut checkpoints = app.world_mut().resource_mut::<ReplicationCheckpointMap>();
        checkpoints.record(replicon_tick, server_tick);
        checkpoints.record_last_confirmed_checkpoint(replicon_tick);
    }

    fn set_interpolation_tick(app: &mut App, tick: Tick) {
        app.world_mut()
            .resource_mut::<InterpolationTimeline>()
            .set_now(TickInstant::from(tick));
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
        app.add_systems(
            Update,
            (
                |mut registry: ResMut<InterpolationRegistry>| registry.finalize(),
                update_interpolation_history,
                apply_interpolation,
            )
                .chain()
                .in_set(crate::plugin::InterpolationSystems::Prepare),
        );
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
        let rule = component_rule::<TestComp, ()>(
            app.world_mut(),
            fns,
            InterpolationRuleConfig { priority: 100 },
            update_history_diff_archetype_erased::<TestComp>,
            insert_confirmed_history_diff::<TestComp>,
        );
        app.world_mut()
            .resource_mut::<InterpolationRegistry>()
            .insert_rule(rule);
    }

    fn insert_rule<C, F>(app: &mut App, fns: InterpolationFns<C>, config: InterpolationRuleConfig)
    where
        C: SyncComponent,
        F: ArchetypeFilter + 'static,
    {
        let rule = component_rule::<C, F>(
            app.world_mut(),
            fns,
            config,
            update_history_archetype_erased::<C>,
            crate::registry::insert_confirmed_history::<C>,
        );
        app.world_mut()
            .resource_mut::<InterpolationRegistry>()
            .insert_rule(rule);
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
    fn disabled_filtered_rule_blocks_broader_default_rule() {
        let mut app = setup_app(Tick(15), 40);
        add_interpolation_test_systems(&mut app);
        QueryState::<&Archetype, With<DisabledRule>>::new(app.world_mut());
        insert_rule::<TestComp, With<DisabledRule>>(
            &mut app,
            InterpolationFns::disabled(),
            InterpolationRuleConfig { priority: 100 },
        );

        let default_entity = app.world_mut().spawn(TestComp(0.0)).id();
        insert_confirmed_history(&mut app, default_entity, two_point_history());
        let disabled_entity = app.world_mut().spawn((TestComp(7.0), DisabledRule)).id();
        insert_confirmed_history(&mut app, disabled_entity, two_point_history());

        app.update();

        assert_eq!(
            app.world().get::<TestComp>(default_entity),
            Some(&TestComp(5.0))
        );
        assert_eq!(
            app.world().get::<TestComp>(disabled_entity),
            Some(&TestComp(7.0))
        );
    }

    /// Checks that a disabled bundle blocks lower-priority rules for every member.
    #[test]
    fn disabled_bundle_blocks_overlapping_component_rules() {
        let mut app = setup_app(Tick(15), 40);
        app.component::<TestComp2>().replicate();
        app.interpolate_with::<TestComp2>(InterpolationFns::interpolate(lerp2));
        app.interpolate_bundle_with::<(TestComp, TestComp2)>(InterpolationFns::disabled());
        add_interpolation_test_systems(&mut app);

        let entity = app
            .world_mut()
            .spawn((TestComp(7.0), TestComp2(9.0), two_point_history2()))
            .id();
        insert_confirmed_history(&mut app, entity, two_point_history());

        app.update();

        assert_eq!(app.world().get::<TestComp>(entity), Some(&TestComp(7.0)));
        assert_eq!(app.world().get::<TestComp2>(entity), Some(&TestComp2(9.0)));
    }

    #[test]
    fn app_linear_interpolate_registers_ease_rule() {
        let mut app = App::new();
        app.add_plugins((
            StatesPlugin,
            RepliconSharedPlugin::default(),
            InterpolationMarkerPlugin,
        ));
        app.world_mut()
            .insert_resource(ReplicationCheckpointMap::default());
        app.component::<TestComp>().replicate();
        app.linear_interpolate::<TestComp>();
        add_interpolation_test_systems(&mut app);

        let mut timeline = InterpolationTimeline::default();
        timeline.set_now(TickInstant::from(Tick(15)));
        timeline.remote_send_interval = core::time::Duration::from_millis(40);
        app.insert_resource(timeline);

        let entity = app.world_mut().spawn(TestComp(0.0)).id();
        insert_confirmed_history(&mut app, entity, two_point_history());

        app.update();

        assert_eq!(app.world().get::<TestComp>(entity), Some(&TestComp(5.0)));
    }

    #[test]
    fn contextual_interpolation_receives_sample_delta() {
        let mut app = setup_app(Tick(15), 40);
        app.world_mut()
            .insert_resource(TickDuration(core::time::Duration::from_millis(50)));
        insert_rule::<TestComp, ()>(
            &mut app,
            InterpolationFns::interpolate_with_context(context_lerp),
            InterpolationRuleConfig { priority: 100 },
        );
        add_interpolation_test_systems(&mut app);

        let entity = app.world_mut().spawn(TestComp(0.0)).id();
        insert_confirmed_history(&mut app, entity, two_point_history());

        app.update();

        assert_eq!(app.world().get::<TestComp>(entity), Some(&TestComp(1.0)));
    }

    #[test]
    fn selected_history_only_rule_suppresses_default_apply() {
        let mut app = setup_app(Tick(15), 40);
        add_interpolation_test_systems(&mut app);
        confirm_server_tick(&mut app, 1, Tick(30));
        QueryState::<&Archetype, With<HistoryOnlyRule>>::new(app.world_mut());
        insert_rule::<TestComp, With<HistoryOnlyRule>>(
            &mut app,
            InterpolationFns::history_only().interpolate(marker_lerp),
            InterpolationRuleConfig { priority: 100 },
        );

        let entity = app.world_mut().spawn((TestComp(7.0), HistoryOnlyRule)).id();
        insert_confirmed_history(&mut app, entity, two_point_history());

        app.update();

        assert_eq!(app.world().get::<TestComp>(entity), Some(&TestComp(7.0)));
        let history = app
            .world()
            .get::<ConfirmedHistory<TestComp>>(entity)
            .unwrap();
        assert_eq!(history.get_nth_tick(history.len() - 1), Some(Tick(30)));
    }

    /// Checks that a winning no-history rule blocks default history and apply work.
    #[test]
    fn selected_no_history_rule_suppresses_default_history_and_apply() {
        let mut app = setup_app(Tick(15), 40);
        add_interpolation_test_systems(&mut app);
        confirm_server_tick(&mut app, 1, Tick(30));
        QueryState::<&Archetype, With<NoHistoryRule>>::new(app.world_mut());
        insert_rule::<TestComp, With<NoHistoryRule>>(
            &mut app,
            InterpolationFns::no_history(marker_lerp),
            InterpolationRuleConfig { priority: 100 },
        );

        let entity = app.world_mut().spawn((TestComp(7.0), NoHistoryRule)).id();
        insert_confirmed_history(&mut app, entity, two_point_history());

        app.update();

        assert_eq!(app.world().get::<TestComp>(entity), Some(&TestComp(7.0)));
        let history = app
            .world()
            .get::<ConfirmedHistory<TestComp>>(entity)
            .unwrap();
        assert_eq!(history.get_nth_tick(history.len() - 1), Some(Tick(20)));
    }

    /// Checks that history-only presence does not make a lower history rule win.
    #[test]
    fn no_history_rule_can_win_when_only_confirmed_history_is_present() {
        let mut app = setup_app(Tick(15), 40);
        add_interpolation_test_systems(&mut app);
        confirm_server_tick(&mut app, 1, Tick(30));
        QueryState::<&Archetype, With<NoHistoryRule>>::new(app.world_mut());
        insert_rule::<TestComp, With<NoHistoryRule>>(
            &mut app,
            InterpolationFns::no_history(marker_lerp),
            InterpolationRuleConfig { priority: 100 },
        );

        let entity = app.world_mut().spawn(NoHistoryRule).id();
        insert_confirmed_history(&mut app, entity, two_point_history());

        app.update();

        assert!(app.world().get::<TestComp>(entity).is_none());
        let history = app
            .world()
            .get::<ConfirmedHistory<TestComp>>(entity)
            .unwrap();
        assert_eq!(history.get_nth_tick(history.len() - 1), Some(Tick(20)));
    }

    #[test]
    fn bundle_interpolation_uses_tuple_interpolation_fn() {
        let mut app = InterpolationTestAppBuilder::new(Tick(15))
            .tick_duration(core::time::Duration::from_millis(100))
            .build();
        app.component::<TestComp>().replicate();
        app.component::<TestComp2>().replicate();
        app.interpolate_bundle_with::<(TestComp, TestComp2)>(InterpolationFns::interpolate(
            bundle_lerp,
        ));
        assert_eq!(
            app.world().resource::<InterpolationRegistry>().rule_count(),
            1,
            "bundle component operations must not be registered as interpolation rules"
        );

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
        app.world_mut().clear_trackers();
        let first_changed = app
            .world()
            .entity(entity)
            .get_change_ticks::<TestComp>()
            .unwrap()
            .changed;
        let second_changed = app
            .world()
            .entity(entity)
            .get_change_ticks::<TestComp2>()
            .unwrap()
            .changed;

        app.update();

        assert_eq!(app.world().get::<TestComp>(entity), Some(&TestComp(105.0)));
        assert_eq!(
            app.world().get::<TestComp2>(entity),
            Some(&TestComp2(205.0))
        );
        assert_ne!(
            app.world()
                .entity(entity)
                .get_change_ticks::<TestComp>()
                .unwrap()
                .changed,
            first_changed
        );
        assert_ne!(
            app.world()
                .entity(entity)
                .get_change_ticks::<TestComp2>()
                .unwrap()
                .changed,
            second_changed
        );
    }

    /// Checks that a missing live bundle member lets its standalone member rule apply.
    #[test]
    fn missing_bundle_member_falls_back_to_standalone_component_rule() {
        let mut app = InterpolationTestAppBuilder::new(Tick(15)).build();
        app.component::<TestComp>().replicate();
        app.component::<TestComp2>().replicate();
        app.interpolate_with::<TestComp2>(InterpolationFns::interpolate(lerp2));
        app.interpolate_bundle_with::<(TestComp, TestComp2)>(InterpolationFns::interpolate(
            bundle_lerp,
        ));

        let entity = app
            .world_mut()
            .spawn((Interpolated, TestComp2(-1.0), two_point_history2()))
            .id();

        app.update();

        assert_eq!(app.world().get::<TestComp2>(entity), Some(&TestComp2(5.0)));
        assert!(!app.world().entity(entity).contains::<TestComp>());
    }

    /// Checks that a selected bundle does not fall back when one owned history is missing.
    #[test]
    fn bundle_rule_does_not_fall_back_when_one_owned_history_is_missing() {
        let mut app = InterpolationTestAppBuilder::new(Tick(15)).build();
        app.component::<TestComp>().replicate();
        app.component::<TestComp2>().replicate();
        app.interpolate_with::<TestComp2>(InterpolationFns::interpolate(lerp2));
        app.interpolate_bundle_with::<(TestComp, TestComp2)>(InterpolationFns::interpolate(
            bundle_lerp,
        ));

        let entity = app
            .world_mut()
            .spawn((
                Interpolated,
                TestComp(-1.0),
                TestComp2(-1.0),
                two_point_history2(),
            ))
            .id();

        app.update();

        // The bundle owns both history initialization and application. Its
        // missing TestComp history prevents it from producing a sample, but
        // the lower-priority TestComp2 rule must not run in its place.
        assert_eq!(app.world().get::<TestComp2>(entity), Some(&TestComp2(-1.0)));
        assert_eq!(app.world().get::<TestComp>(entity), Some(&TestComp(-1.0)));
    }

    /// Checks independent history and apply ownership across delayed bundle-member removal.
    #[test]
    fn bundle_maintains_history_while_component_rule_applies_after_member_removal() {
        let mut app = InterpolationTestAppBuilder::new(Tick(15)).build();
        app.component::<TestComp>().replicate();
        app.component::<TestComp2>().replicate();
        app.interpolate_with::<TestComp>(InterpolationFns::interpolate(lerp));
        app.interpolate_bundle_with::<(TestComp, TestComp2)>(InterpolationFns::interpolate(
            bundle_lerp,
        ));

        let mut first = ConfirmedHistory::<TestComp>::default();
        first.insert_present(Tick(10), TestComp(0.0));
        first.insert_present(Tick(20), TestComp(10.0));
        let mut second = ConfirmedHistory::<TestComp2>::default();
        second.insert_present(Tick(10), TestComp2(3.0));
        second.insert_removed(Tick(20));
        let entity = app
            .world_mut()
            .spawn((Interpolated, TestComp(-1.0), TestComp2(-1.0), first, second))
            .id();

        app.update();

        assert_eq!(app.world().get::<TestComp>(entity), Some(&TestComp(105.0)));
        assert_eq!(
            app.world().get::<TestComp2>(entity),
            Some(&TestComp2(203.0))
        );

        set_interpolation_tick(&mut app, Tick(20));
        app.update();

        // The bundle keeps maintaining both histories, but it no longer owns
        // apply once TestComp2 is absent. The standalone TestComp rule takes
        // over immediately at the removal boundary.
        assert_eq!(app.world().get::<TestComp>(entity), Some(&TestComp(10.0)));
        assert!(!app.world().entity(entity).contains::<TestComp2>());
    }

    /// Checks independent history and apply ownership across delayed bundle-member insertion.
    #[test]
    fn bundle_maintains_history_while_component_rule_applies_before_delayed_insert() {
        let mut app = InterpolationTestAppBuilder::new(Tick(15)).build();
        app.component::<TestComp>().replicate();
        app.component::<TestComp2>().replicate();
        app.interpolate_with::<TestComp>(InterpolationFns::interpolate(lerp));
        app.interpolate_bundle_with::<(TestComp, TestComp2)>(InterpolationFns::interpolate(
            bundle_lerp,
        ));

        let mut first = ConfirmedHistory::<TestComp>::default();
        first.insert_present(Tick(10), TestComp(0.0));
        first.insert_present(Tick(20), TestComp(10.0));
        first.insert_present(Tick(30), TestComp(20.0));
        let mut second = ConfirmedHistory::<TestComp2>::default();
        second.insert_removed(Tick(10));
        second.insert_present(Tick(20), TestComp2(4.0));
        second.insert_present(Tick(30), TestComp2(8.0));
        let entity = app
            .world_mut()
            .spawn((Interpolated, TestComp(-1.0), first, second))
            .id();

        app.update();

        assert_eq!(app.world().get::<TestComp>(entity), Some(&TestComp(5.0)));
        assert!(!app.world().entity(entity).contains::<TestComp2>());

        set_interpolation_tick(&mut app, Tick(20));
        app.update();

        // History maintenance inserts TestComp2 before apply resolution runs
        // again, so the bundle takes over in the same update.
        assert_eq!(
            app.world().get::<TestComp2>(entity),
            Some(&TestComp2(204.0))
        );
        assert_eq!(app.world().get::<TestComp>(entity), Some(&TestComp(110.0)));
    }

    #[test]
    fn bundle_contextual_interpolation_receives_sample_delta() {
        let mut app = InterpolationTestAppBuilder::new(Tick(15))
            .tick_duration(core::time::Duration::from_millis(100))
            .build();
        app.component::<TestComp>().replicate();
        app.component::<TestComp2>().replicate();
        app.interpolate_bundle_with::<(TestComp, TestComp2)>(
            InterpolationFns::interpolate_with_context(bundle_context_lerp),
        );

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

        assert_eq!(app.world().get::<TestComp>(entity), Some(&TestComp(0.5)));
        assert_eq!(app.world().get::<TestComp2>(entity), Some(&TestComp2(1.0)));
    }

    #[test]
    fn bundle_interpolation_inserts_tuple_interpolated_components() {
        let mut app = InterpolationTestAppBuilder::new(Tick(15)).build();
        app.component::<TestComp>().replicate();
        app.component::<TestComp2>().replicate();
        app.interpolate_bundle_with::<(TestComp, TestComp2)>(InterpolationFns::interpolate(
            bundle_lerp,
        ));

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

        let mut app = InterpolationTestAppBuilder::new(Tick(15)).build();
        app.component::<TestComp>().replicate();
        app.component::<TestComp2>().replicate();
        app.component::<TestBundleComp<3>>().replicate();
        app.interpolate_bundle_with::<(TestComp, TestComp2)>(InterpolationFns::interpolate(
            bundle2_priority_lerp,
        ));
        app.interpolate_bundle_with::<(TestComp, TestComp2, TestBundleComp<3>)>(
            InterpolationFns::interpolate(bundle3_priority_lerp),
        );

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
        let mut app = InterpolationTestAppBuilder::new(Tick(15)).build();
        app.component::<TestComp>().replicate();
        app.component::<TestComp2>().replicate();
        app.interpolate_with_priority::<TestComp>(2, InterpolationFns::history_only());
        app.interpolate_bundle_with::<(TestComp, TestComp2)>(InterpolationFns::interpolate(
            bundle_lerp,
        ));
        confirm_server_tick(&mut app, 1, Tick(30));

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
        let first_history = app
            .world()
            .get::<ConfirmedHistory<TestComp>>(entity)
            .unwrap();
        assert_eq!(
            first_history.get_nth_tick(first_history.len() - 1),
            Some(Tick(30))
        );
        let second_history = app
            .world()
            .get::<ConfirmedHistory<TestComp2>>(entity)
            .unwrap();
        assert_eq!(
            second_history.get_nth_tick(second_history.len() - 1),
            Some(Tick(20))
        );
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

        let mut app = InterpolationTestAppBuilder::new(Tick(15)).build();
        app.component::<TestBundleComp<1>>().replicate();
        app.component::<TestBundleComp<2>>().replicate();
        app.component::<TestBundleComp<3>>().replicate();
        app.component::<TestBundleComp<4>>().replicate();
        app.component::<TestBundleComp<5>>().replicate();
        app.component::<TestBundleComp<6>>().replicate();
        app.component::<TestBundleComp<7>>().replicate();
        app.component::<TestBundleComp<8>>().replicate();
        app.interpolate_bundle_with::<Bundle8>(InterpolationFns::interpolate(bundle8_lerp));

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
        add_interpolation_test_systems(&mut app);
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
    fn diff_history_waits_when_completed_tick_diff_is_pending() {
        let mut app = setup_app(Tick(5), 40);
        use_diff_history_rule(&mut app);
        add_interpolation_test_systems(&mut app);
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
        add_interpolation_test_systems(&mut app);

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
    fn update_confirmed_history_diff_waits_for_older_pending_diff() {
        let mut app = setup_app(Tick(6), 40);
        use_diff_history_rule(&mut app);
        add_interpolation_test_systems(&mut app);
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
        assert_eq!(history.len(), 1);
        assert_eq!(
            history.start_present().map(|(t, v)| (t, v.clone())),
            Some((Tick(0), TestComp(0.0)))
        );
        assert!(history.get_state_at(Tick(6)).is_none());

        let receiver = app
            .world()
            .resource::<ReplicationStorage>()
            .get::<HistoryDiffReceiver<TestComp>>(entity)
            .unwrap();
        assert!(receiver.has_pending_diffs());
        assert_eq!(receiver.tick_for_cursor(Some(idx(0))), Some(Tick(0)));

        // Materialize the missing S0 -> S3 base and the buffered S3 -> S5 diff. The next update
        // revisits the still-latest completed checkpoint and can now add its unchanged anchor.
        let mut receiver = app
            .world_mut()
            .resource_mut::<ReplicationStorage>()
            .remove::<HistoryDiffReceiver<TestComp>>(entity)
            .unwrap();
        {
            let mut entity_mut = app.world_mut().entity_mut(entity);
            let mut history = entity_mut.get_mut::<ConfirmedHistory<TestComp>>().unwrap();
            receiver
                .queue_diffs(Tick(3), idx(1), vec![1.0, 2.0, 3.0])
                .unwrap();
            while let Some((tick, value)) = receiver.take_ready_update(&history).unwrap() {
                history.insert_present(tick, value);
            }
        }
        assert!(!receiver.has_pending_diffs());
        app.world_mut()
            .resource_mut::<ReplicationStorage>()
            .insert(entity, receiver);

        app.update();

        let history = app
            .world()
            .get::<ConfirmedHistory<TestComp>>(entity)
            .unwrap();
        assert_eq!(
            history.get_state_at(Tick(6)).and_then(HistoryState::value),
            Some(&TestComp(5.0))
        );
    }

    #[test]
    fn update_confirmed_history_does_not_move_history_backwards() {
        let mut app = setup_app(Tick(30), 40);
        add_interpolation_test_systems(&mut app);
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
    fn update_confirmed_history_keeps_bracketing_pair_during_loss_gap() {
        let mut app = setup_app(Tick(25), 40);
        add_interpolation_test_systems(&mut app);

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
        add_interpolation_test_systems(&mut app);

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
        add_interpolation_test_systems(&mut app);

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
        add_interpolation_test_systems(&mut app);

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
        add_interpolation_test_systems(&mut app);

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
        add_interpolation_test_systems(&mut app);

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
}
