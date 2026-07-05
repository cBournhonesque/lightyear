//! This module provides the ability to smooth the rollback (from the Predicted state to the Corrected state) over a period
//! of time instead of just snapping back instantly to the Corrected state. This might help hide rollback artifacts.
//! We will call the interpolated state the Visual state.
//!
//! For example the current tick is 10, and you have a predicted value P10.
//! You receive a confirmed update C5 for tick 5, which doesn't match with the value we had stored in the
//! prediction history at tick 5. This means we need to rollback.
//!
//! Without correction, we would simply snap back the value at tick 5 to C5, and then re-run the simulation
//! from tick 5 to 10 to get a new value C10 at tick 10. The simulation will visually snap back from (predicted) P10 to (corrected) C10.
//! Instead what we can do is correct the value from P10 to C10 over a period of time.
//!
//! The flow is (if T is the tick for the start of the rollback, and X is the current tick)
//! - PreUpdate: we see that there is a rollback needed. We insert
//!   Correction { original_value = PT, start_tick, end_tick }
//! - RunRollback, which lets us compute the correct CT value.
//! - FixedUpdate: we run the simulation to get the new value C(T+1)
//! - FixedPostUpdate: set the component value to the interpolation between PT and C(T+1)
//!
//! - PreUpdate: restore the C(T+1) value (corrected value at the current tick T+1)
//!   - if there is a rollback, restart correction from the current corrected value
//! - FixedUpdate: run the simulation to compute C(T+2).
//! - FixedPostUpdate: set the component value to the interpolation between PT (predicted value at rollback start T) and C(T+2)

use crate::SyncComponent;
use crate::manager::PredictionManager;
use crate::predicted_history::PredictionHistory;
use crate::registry::PredictionRegistry;
use crate::rollback::RollbackSystems;
use alloc::vec::Vec;
use bevy_app::prelude::*;
use bevy_ecs::{
    archetype::Archetype,
    change_detection::Tick as ChangeTick,
    component::{ComponentId, StorageType},
    prelude::*,
    query::{FilteredAccess, FilteredAccessSet},
    storage::Table,
    system::{SystemMeta, SystemParam, SystemParamValidationError},
    world::unsafe_world_cell::UnsafeWorldCell,
};
use bevy_reflect::Reflect;
use bevy_time::{Fixed, Time, Virtual};
use bevy_utils::prelude::DebugName;
use core::cell::UnsafeCell;
use core::fmt::Debug;
use lightyear_core::prelude::{FrameInterpolationHistory, LocalTimeline, Tick};
use lightyear_frame_interpolation::FrameInterpolationSystems;
use lightyear_interpolation::registry::InterpolationRegistry;
use lightyear_replication::deferred_entity::DeferredEntityCommands;
use lightyear_replication::delta::Diffable;
use lightyear_replication::registry::ComponentKind;
use tracing::trace;

/// The visual value of the component before the rollback started
#[derive(Component, Debug, Reflect)]
pub struct PreviousVisual<C: Component>(pub C);

#[derive(Component, Debug, Reflect)]
pub struct VisualCorrection<D> {
    /// The error between the original visual value and the new visual value.
    /// Will decay over time.
    pub error: D,
}

/// Context shared by type-erased post-rollback correction handlers.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PostRollbackCorrectionContext {
    tick: Tick,
    overstep: f32,
}

/// Type-erased post-rollback correction handler registered per corrected component.
pub(crate) type ErasedUpdatePostRollbackFrameHistoryFn = fn(
    UnsafeWorldCell,
    &ErasedPostRollbackCorrection,
    PostRollbackCorrectionContext,
    &mut DeferredEntityCommands,
);

/// Type-erased visual correction handler registered per corrected component.
pub(crate) type ErasedCreateVisualCorrectionFn = fn(
    UnsafeWorldCell,
    &ErasedPostRollbackCorrection,
    PostRollbackCorrectionContext,
    &mut DeferredEntityCommands,
);

/// Type-erased post-rollback frame-history restore handler.
pub(crate) type ErasedRestorePostRollbackFrameHistoryFn =
    fn(UnsafeWorldCell, &ErasedPostRollbackCorrection);

/// Type-erased post-rollback correction metadata registered for one component.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ErasedPostRollbackCorrection {
    kind: ComponentKind,
    update_history: ErasedUpdatePostRollbackFrameHistoryFn,
    create_visual_correction: ErasedCreateVisualCorrectionFn,
    restore_history: ErasedRestorePostRollbackFrameHistoryFn,
    live_component_id: ComponentId,
    previous_visual_component_id: ComponentId,
    prediction_history_component_id: ComponentId,
    frame_history_component_id: ComponentId,
}

impl ErasedPostRollbackCorrection {
    pub(crate) fn new<C, D>(world: &mut World) -> Self
    where
        C: SyncComponent + Diffable<D>,
        D: Debug + Send + Sync + 'static,
    {
        Self {
            kind: ComponentKind::of::<C>(),
            update_history: update_frame_history_post_rollback_erased::<C, D>,
            create_visual_correction: create_visual_correction_from_live_erased::<C, D>,
            restore_history: restore_frame_history_post_rollback_erased::<C, D>,
            live_component_id: world.register_component::<C>(),
            previous_visual_component_id: world.register_component::<PreviousVisual<C>>(),
            prediction_history_component_id: world.register_component::<PredictionHistory<C>>(),
            frame_history_component_id: world.register_component::<FrameInterpolationHistory<C>>(),
        }
    }

    pub(crate) fn kind(&self) -> ComponentKind {
        self.kind
    }

    fn add_access(&self, filtered_access: &mut FilteredAccess) {
        filtered_access.add_write(self.live_component_id);
        filtered_access.add_read(self.previous_visual_component_id);
        filtered_access.add_read(self.prediction_history_component_id);
        filtered_access.add_write(self.frame_history_component_id);
    }

    fn update(
        &self,
        world: UnsafeWorldCell,
        ctx: PostRollbackCorrectionContext,
        deferred_apply: &mut DeferredEntityCommands,
    ) {
        (self.update_history)(world, self, ctx, deferred_apply);
    }

    fn create_visual_correction(
        &self,
        world: UnsafeWorldCell,
        ctx: PostRollbackCorrectionContext,
        deferred_apply: &mut DeferredEntityCommands,
    ) {
        (self.create_visual_correction)(world, self, ctx, deferred_apply);
    }

    fn restore_history(&self, world: UnsafeWorldCell) {
        (self.restore_history)(world, self);
    }
}

/// System param exposing a low-level world cell for post-rollback correction.
///
/// Access is declared from the erased correction handlers registered in
/// [`PredictionRegistry`], so the dispatcher can scan component columns without
/// taking `&mut World`.
pub(crate) struct PostRollbackCorrectionWorld<'w> {
    world: UnsafeWorldCell<'w>,
}

unsafe impl SystemParam for PostRollbackCorrectionWorld<'_> {
    type State = ();
    type Item<'world, 'state> = PostRollbackCorrectionWorld<'world>;

    fn init_state(_world: &mut World) -> Self::State {}

    fn init_access(
        _state: &Self::State,
        _system_meta: &mut SystemMeta,
        component_access_set: &mut FilteredAccessSet,
        world: &mut World,
    ) {
        let mut filtered_access = FilteredAccess::default();
        if let Some(registry) = world.get_resource::<PredictionRegistry>() {
            for correction in registry.post_rollback_corrections() {
                correction.add_access(&mut filtered_access);
            }
        }
        if let Some(registry) = world.get_resource::<InterpolationRegistry>() {
            for component_id in registry.frame_component_write_ids() {
                filtered_access.add_write(component_id);
            }
        }
        component_access_set.add(filtered_access);
    }

    unsafe fn get_param<'world, 'state>(
        _state: &'state mut Self::State,
        _system_meta: &SystemMeta,
        world: UnsafeWorldCell<'world>,
        _change_tick: ChangeTick,
    ) -> Result<Self::Item<'world, 'state>, SystemParamValidationError> {
        Ok(PostRollbackCorrectionWorld { world })
    }
}

#[derive(Resource, Default)]
struct PostRollbackCorrectionSystemInstalled;

pub fn add_correction_systems<
    C: SyncComponent + Diffable<D>,
    D: Default + Clone + Debug + Send + Sync + 'static,
>(
    app: &mut App,
) {
    // When rollback finishes, compute the new corrected visual value and compare it with the original visual value
    // to set the visual correction error.
    if !app
        .world()
        .contains_resource::<PostRollbackCorrectionSystemInstalled>()
    {
        app.insert_resource(PostRollbackCorrectionSystemInstalled);
        app.add_systems(
            PreUpdate,
            update_frame_interpolation_post_rollback.in_set(RollbackSystems::EndRollback),
        );
    }
    app.configure_sets(
        PostUpdate,
        // If FrameInterpolation runs after Correction, it would overwrite the applied correction.
        RollbackSystems::VisualCorrection.after(FrameInterpolationSystems::Interpolate),
    );
    app.add_systems(
        PostUpdate,
        add_visual_correction::<C, D>.in_set(RollbackSystems::VisualCorrection),
    );
}

/// After the rollback is over, we need to update the values in the [`FrameInterpolationHistory<C>`] component.
/// This is important to run now and not in FixedUpdate because FixedUpdate could not run this frame.
/// (if we have two frames in a row)
///
/// If we have correction enabled, then we can compute the error between the previous visual value
/// [`PreviousVisual<C>`] and the new visual value.
pub(crate) fn update_frame_interpolation_post_rollback(
    time: Res<Time<Fixed>>,
    timeline: Res<LocalTimeline>,
    prediction_registry: Res<PredictionRegistry>,
    interpolation_registry: Res<InterpolationRegistry>,
    correction_world: PostRollbackCorrectionWorld,
    mut commands: Commands,
) {
    let ctx = PostRollbackCorrectionContext {
        // NOTE: this is the overstep from the previous frame since we are running this before RunFixedMainLoop
        overstep: time.overstep_fraction(),
        tick: timeline.tick(),
    };
    let mut deferred_apply = DeferredEntityCommands::default();
    let world = correction_world.world;
    let corrections = prediction_registry
        .post_rollback_corrections()
        .collect::<Vec<_>>();
    for correction in &corrections {
        correction.update(world, ctx, &mut deferred_apply);
    }
    apply_frame_interpolation_for_visual_correction(
        world,
        &corrections,
        &interpolation_registry,
        ctx,
        &mut deferred_apply,
    );
    for correction in &corrections {
        correction.create_visual_correction(world, ctx, &mut deferred_apply);
    }
    for correction in &corrections {
        correction.restore_history(world);
    }
    deferred_apply.apply(&mut commands);
}

pub(crate) fn update_frame_history_post_rollback_erased<
    C: SyncComponent + Diffable<D>,
    D: Debug + Send + Sync + 'static,
>(
    world: UnsafeWorldCell,
    correction: &ErasedPostRollbackCorrection,
    ctx: PostRollbackCorrectionContext,
    _deferred_apply: &mut DeferredEntityCommands,
) {
    let component_id = correction.live_component_id;
    let prediction_history_id = correction.prediction_history_component_id;
    let frame_history_id = correction.frame_history_component_id;

    for archetype in world.archetypes().iter().filter(|archetype| {
        archetype.contains(component_id)
            && archetype.contains(prediction_history_id)
            && archetype.contains(frame_history_id)
    }) {
        let Some(StorageType::Table) = archetype.get_storage_type(component_id) else {
            continue;
        };
        let Some(StorageType::Table) = archetype.get_storage_type(prediction_history_id) else {
            continue;
        };
        let Some(StorageType::Table) = archetype.get_storage_type(frame_history_id) else {
            continue;
        };
        let Some(table) = table_for_archetype(world, archetype) else {
            continue;
        };
        let Some(components) = table_component_slice::<C>(table, component_id) else {
            continue;
        };
        let Some(prediction_histories) =
            table_component_slice::<PredictionHistory<C>>(table, prediction_history_id)
        else {
            continue;
        };
        let Some(frame_histories) =
            table_component_slice::<FrameInterpolationHistory<C>>(table, frame_history_id)
        else {
            continue;
        };
        for entity in archetype.entities() {
            let row = entity.table_row().index();
            let component = unsafe { &*components.get_unchecked(row).get() };
            let history = unsafe { &*prediction_histories.get_unchecked(row).get() };
            let interpolate = unsafe { &mut *frame_histories.get_unchecked(row).get() };

            // update the FrameInterpolation with the last 2 history values
            interpolate.current_value = Some(component.clone());
            interpolate.previous_value = history.get(ctx.tick - 1).cloned();
        }
    }
}

fn apply_frame_interpolation_for_visual_correction(
    world: UnsafeWorldCell,
    corrections: &[ErasedPostRollbackCorrection],
    interpolation_registry: &InterpolationRegistry,
    ctx: PostRollbackCorrectionContext,
    deferred_apply: &mut DeferredEntityCommands,
) {
    for archetype in world
        .archetypes()
        .iter()
        .filter(|archetype| archetype_has_previous_visual(archetype, corrections))
    {
        let active_correction_members = corrections
            .iter()
            .filter_map(|correction| {
                archetype
                    .contains(correction.previous_visual_component_id)
                    .then_some(correction.kind())
            })
            .collect::<Vec<_>>();
        let mut selected_rules = Vec::new();
        for kind in interpolation_registry.rule_kinds() {
            if let Some(rule_id) = interpolation_registry.select_rule_for_archetype(
                world.components(),
                archetype,
                kind,
            ) {
                selected_rules.push(rule_id);
            }
        }
        selected_rules.sort_by(|lhs, rhs| interpolation_registry.cmp_rule_precedence(*lhs, *rhs));

        let mut claimed_members = Vec::new();
        for rule_id in selected_rules {
            let Some(rule) = interpolation_registry.rule(rule_id) else {
                continue;
            };
            if rule
                .members()
                .iter()
                .any(|member| !active_correction_members.contains(member))
            {
                continue;
            }
            if rule
                .members()
                .iter()
                .any(|member| claimed_members.contains(member))
            {
                continue;
            }
            claimed_members.extend(rule.members().iter().copied());
            if let Some(apply) = interpolation_registry.cached_frame_apply_component(rule_id) {
                (apply.apply_frame_interpolation())(
                    world,
                    archetype,
                    interpolation_registry,
                    apply.rule_id(),
                    interpolation_context(ctx),
                    false,
                    deferred_apply,
                );
            }
        }
    }
}

fn archetype_has_previous_visual(
    archetype: &Archetype,
    corrections: &[ErasedPostRollbackCorrection],
) -> bool {
    corrections
        .iter()
        .any(|correction| archetype.contains(correction.previous_visual_component_id))
}

fn interpolation_context(
    ctx: PostRollbackCorrectionContext,
) -> lightyear_interpolation::rules::frame_interpolate::FrameInterpolationContext {
    lightyear_interpolation::rules::frame_interpolate::FrameInterpolationContext {
        overstep: ctx.overstep,
    }
}

pub(crate) fn create_visual_correction_from_live_erased<
    C: SyncComponent + Diffable<D>,
    D: Debug + Send + Sync + 'static,
>(
    world: UnsafeWorldCell,
    correction: &ErasedPostRollbackCorrection,
    ctx: PostRollbackCorrectionContext,
    deferred_apply: &mut DeferredEntityCommands,
) {
    let component_id = correction.live_component_id;
    let frame_history_id = correction.frame_history_component_id;
    let previous_visual_id = correction.previous_visual_component_id;

    for archetype in world.archetypes().iter().filter(|archetype| {
        archetype.contains(component_id)
            && archetype.contains(frame_history_id)
            && archetype.contains(previous_visual_id)
    }) {
        let Some(StorageType::Table) = archetype.get_storage_type(component_id) else {
            continue;
        };
        let Some(StorageType::Table) = archetype.get_storage_type(frame_history_id) else {
            continue;
        };
        let Some(StorageType::Table) = archetype.get_storage_type(previous_visual_id) else {
            continue;
        };
        let Some(table) = table_for_archetype(world, archetype) else {
            continue;
        };
        let Some(components) = table_component_slice::<C>(table, component_id) else {
            continue;
        };
        let Some(frame_histories) =
            table_component_slice::<FrameInterpolationHistory<C>>(table, frame_history_id)
        else {
            continue;
        };
        let Some(previous_visuals) =
            table_component_slice::<PreviousVisual<C>>(table, previous_visual_id)
        else {
            continue;
        };

        for entity in archetype.entities() {
            let entity_id = entity.id();
            let row = entity.table_row().index();
            let current_visual = unsafe { &*components.get_unchecked(row).get() };
            let interpolate = unsafe { &*frame_histories.get_unchecked(row).get() };
            if interpolate.previous_value.is_none() {
                continue;
            }
            let previous_visual = unsafe { &*previous_visuals.get_unchecked(row).get() };
            // error = previous_visual - current_visual
            let error = current_visual.diff(&previous_visual.0);
            trace!(
                target: "lightyear_debug::prediction",
                kind = "visual_correction_created",
                schedule = "PreUpdate",
                sample_point = "PreUpdate",
                entity = ?entity_id,
                component = ?DebugName::type_name::<C>(),
                local_tick = ctx.tick.0,
                overstep = ctx.overstep,
                current_visual = ?current_visual,
                previous_visual = ?previous_visual,
                error = ?error,
                "created visual correction after rollback"
            );
            deferred_apply.insert(entity_id, VisualCorrection::<D> { error });
            deferred_apply.remove::<PreviousVisual<C>>(entity_id);
        }
    }
}

pub(crate) fn restore_frame_history_post_rollback_erased<
    C: SyncComponent + Diffable<D>,
    D: Debug + Send + Sync + 'static,
>(
    world: UnsafeWorldCell,
    correction: &ErasedPostRollbackCorrection,
) {
    let component_id = correction.live_component_id;
    let frame_history_id = correction.frame_history_component_id;

    for archetype in world.archetypes().iter().filter(|archetype| {
        archetype.contains(component_id) && archetype.contains(frame_history_id)
    }) {
        let Some(StorageType::Table) = archetype.get_storage_type(component_id) else {
            continue;
        };
        let Some(StorageType::Table) = archetype.get_storage_type(frame_history_id) else {
            continue;
        };
        let Some(table) = table_for_archetype(world, archetype) else {
            continue;
        };
        let Some(components) = table_component_slice::<C>(table, component_id) else {
            continue;
        };
        let Some(frame_histories) =
            table_component_slice::<FrameInterpolationHistory<C>>(table, frame_history_id)
        else {
            continue;
        };

        for entity in archetype.entities() {
            let row = entity.table_row().index();
            let interpolate = unsafe { &*frame_histories.get_unchecked(row).get() };
            let Some(current_value) = &interpolate.current_value else {
                continue;
            };
            let component = unsafe { &mut *components.get_unchecked(row).get() };
            *component = current_value.clone();
        }
    }
}

fn table_for_archetype<'w>(world: UnsafeWorldCell<'w>, archetype: &Archetype) -> Option<&'w Table> {
    unsafe { world.storages().tables.get(archetype.table_id()) }
}

fn table_component_slice<C: Component>(
    table: &Table,
    component_id: ComponentId,
) -> Option<&[UnsafeCell<C>]> {
    unsafe { table.get_data_slice_for::<C>(component_id) }
}

/// Add the visual correction error to the visual component, and
/// decay the visual correction error over time.
///
/// If it gets small enough, we remove the `VisualCorrection<C>` component.
///
/// The component `C` must have an interpolation function registered in the
/// [`InterpolationRegistry`]. Correction reuses that interpolation rule to
/// decay the visual error instead of storing a prediction-specific correction
/// function.
pub(crate) fn add_visual_correction<
    C: SyncComponent + Diffable<D>,
    D: Default + Clone + Debug + Send + Sync + 'static,
>(
    time: Res<Time<Virtual>>,
    prediction: Res<PredictionRegistry>,
    interpolation_registry: Res<InterpolationRegistry>,
    manager: Single<&PredictionManager>,
    mut query: Query<(Entity, &mut C, &mut VisualCorrection<D>)>,
    mut commands: Commands,
) {
    let r = manager.correction_policy.lerp_ratio(time.delta());
    let interpolation = interpolation_registry.interpolation_for::<C>().expect(
        "No interpolation function was found for correction. Register an interpolation rule for this component before calling add_linear_correction_fn.",
    );
    query
        .iter_mut()
        .for_each(|(entity, mut component, mut visual_correction)| {
            let previous_error = visual_correction.error.clone();
            let mut error_as_component = C::base_value();
            error_as_component.apply_diff(&previous_error);
            if !prediction.should_rollback(&C::base_value(), &error_as_component) {
                trace!(
                    target: "lightyear_debug::prediction",
                    kind = "visual_correction_removed",
                    schedule = "PostUpdate",
                    sample_point = "PostUpdate",
                    entity = ?entity,
                    component = ?DebugName::type_name::<C>(),
                    error = ?visual_correction.error,
                    "removed visual correction because error is small"
                );
                commands.entity(entity).remove::<VisualCorrection<D>>();
                return;
            }
            let new_error_component = interpolation(C::base_value(), error_as_component, r);
            let new_error = C::base_value().diff(&new_error_component);
            component.bypass_change_detection().apply_diff(&new_error);
            trace!(
                target: "lightyear_debug::prediction",
                kind = "visual_correction_apply",
                schedule = "PostUpdate",
                sample_point = "PostUpdate",
                entity = ?entity,
                component = ?DebugName::type_name::<C>(),
                previous_error = ?previous_error,
                new_error = ?new_error,
                ratio = r,
                "applied visual correction"
            );
            visual_correction.error = new_error;
        });
}

#[derive(Component, Debug, Reflect)]
pub struct CorrectionPolicy {
    /// Period of time to decay the error by `decay_ratio`
    decay_period: core::time::Duration,
    /// Fraction of the error remaining after `decay_period` has passed.
    ///
    /// For example if `decay_period` is 1 second and `decay_ratio` is 0.3, then only 30% of the original error
    /// remains after 1 second.
    decay_ratio: f32,
    /// We will stop applying correction after this amount of time has passed since the rollback started.
    max_correction_period: core::time::Duration,
}

impl Default for CorrectionPolicy {
    fn default() -> Self {
        Self {
            decay_period: core::time::Duration::from_millis(200),
            decay_ratio: 0.5,
            max_correction_period: core::time::Duration::from_secs(600),
        }
    }
}

impl CorrectionPolicy {
    /// Returns the lerp constant to use for exponentially decaying the error in a framestep-insensitive way
    ///
    /// See: <https://www.youtube.com/watch?v=LSNQuFEDOyQ>
    #[inline]
    pub fn lerp_ratio(&self, delta: core::time::Duration) -> f32 {
        let dt = delta.as_secs_f32();
        let neg_decay_constant = self.decay_ratio.ln() / self.decay_period.as_secs_f32();
        (neg_decay_constant * dt).exp()
    }

    pub fn instant_correction() -> Self {
        Self {
            decay_period: core::time::Duration::from_millis(1),
            decay_ratio: 0.0000001,
            max_correction_period: core::time::Duration::from_millis(10),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::{PredictionBuilderExt, PredictionRegistry};
    use bevy_ecs::system::RunSystemOnce;
    use bevy_math::{
        Curve,
        curve::{Ease, FunctionCurve, Interval},
    };
    use bevy_replicon::prelude::{AuthMethod, RepliconSharedPlugin};
    use bevy_state::app::StatesPlugin;
    use core::time::Duration;
    use lightyear_core::prelude::FrameInterpolationHistory;
    use lightyear_interpolation::registry::AppInterpolationExt;
    use lightyear_interpolation::rules::InterpolationFns;
    use lightyear_replication::prelude::AppComponentExt;

    #[derive(Component, Clone, Debug, Default, PartialEq)]
    struct CorrectionA(f32);

    #[derive(Component, Clone, Debug, Default, PartialEq)]
    struct CorrectionB(f32);

    impl Ease for CorrectionA {
        fn interpolating_curve_unbounded(start: Self, end: Self) -> impl Curve<Self> {
            FunctionCurve::new(Interval::UNIT, move |t| {
                CorrectionA(start.0 + (end.0 - start.0) * t)
            })
        }
    }

    impl Ease for CorrectionB {
        fn interpolating_curve_unbounded(start: Self, end: Self) -> impl Curve<Self> {
            FunctionCurve::new(Interval::UNIT, move |t| {
                CorrectionB(start.0 + (end.0 - start.0) * t)
            })
        }
    }

    impl Diffable<CorrectionA> for CorrectionA {
        fn base_value() -> Self {
            Self::default()
        }

        fn diff(&self, new: &Self) -> CorrectionA {
            CorrectionA(new.0 - self.0)
        }

        fn apply_diff(&mut self, delta: &CorrectionA) {
            self.0 += delta.0;
        }
    }

    impl Diffable<CorrectionB> for CorrectionB {
        fn base_value() -> Self {
            Self::default()
        }

        fn diff(&self, new: &Self) -> CorrectionB {
            CorrectionB(new.0 - self.0)
        }

        fn apply_diff(&mut self, delta: &CorrectionB) {
            self.0 += delta.0;
        }
    }

    fn bundle_lerp(
        start: (CorrectionA, CorrectionB),
        end: (CorrectionA, CorrectionB),
        t: f32,
    ) -> (CorrectionA, CorrectionB) {
        (
            CorrectionA(100.0 + start.0.0 + (end.0.0 - start.0.0) * t),
            CorrectionB(200.0 + start.1.0 + (end.1.0 - start.1.0) * t),
        )
    }

    #[test]
    fn post_rollback_correction_uses_bundle_interpolation_rule() {
        let mut app = App::new();
        app.add_plugins((
            StatesPlugin,
            RepliconSharedPlugin {
                auth_method: AuthMethod::None,
            },
        ));
        app.init_resource::<PredictionRegistry>();
        app.insert_resource(Time::<Fixed>::from_duration(Duration::from_secs(1)));
        app.world_mut()
            .resource_mut::<Time<Fixed>>()
            .accumulate_overstep(Duration::from_millis(500));
        app.insert_resource(LocalTimeline::default());
        app.world_mut()
            .resource_mut::<LocalTimeline>()
            .apply_delta(10);

        app.component::<CorrectionA>()
            .predict()
            .add_linear_correction_fn::<CorrectionA>();
        app.component::<CorrectionB>()
            .predict()
            .add_linear_correction_fn::<CorrectionB>();

        app.interpolate_with::<CorrectionA>(InterpolationFns::no_history(|_, _, _| {
            CorrectionA(1_000.0)
        }));
        app.interpolate_with::<CorrectionB>(InterpolationFns::no_history(|_, _, _| {
            CorrectionB(2_000.0)
        }));
        app.interpolate_bundle_with::<(CorrectionA, CorrectionB)>(InterpolationFns::no_history(
            bundle_lerp,
        ));

        let mut history_a = PredictionHistory::<CorrectionA>::default();
        history_a.add_predicted(Tick(9), Some(CorrectionA(0.0)));
        let mut history_b = PredictionHistory::<CorrectionB>::default();
        history_b.add_predicted(Tick(9), Some(CorrectionB(0.0)));

        let entity = app
            .world_mut()
            .spawn((
                CorrectionA(10.0),
                CorrectionB(20.0),
                PreviousVisual(CorrectionA(1.0)),
                PreviousVisual(CorrectionB(2.0)),
                history_a,
                history_b,
                FrameInterpolationHistory::<CorrectionA>::default(),
                FrameInterpolationHistory::<CorrectionB>::default(),
            ))
            .id();

        app.world_mut()
            .run_system_once(update_frame_interpolation_post_rollback)
            .unwrap();
        app.world_mut().flush();

        assert_eq!(
            app.world().get::<CorrectionA>(entity),
            Some(&CorrectionA(10.0))
        );
        assert_eq!(
            app.world().get::<CorrectionB>(entity),
            Some(&CorrectionB(20.0))
        );
        assert_eq!(
            app.world()
                .get::<VisualCorrection<CorrectionA>>(entity)
                .map(|correction| &correction.error),
            Some(&CorrectionA(-104.0))
        );
        assert_eq!(
            app.world()
                .get::<VisualCorrection<CorrectionB>>(entity)
                .map(|correction| &correction.error),
            Some(&CorrectionB(-208.0))
        );
        assert!(
            app.world()
                .get::<PreviousVisual<CorrectionA>>(entity)
                .is_none()
        );
        assert!(
            app.world()
                .get::<PreviousVisual<CorrectionB>>(entity)
                .is_none()
        );
    }
}
