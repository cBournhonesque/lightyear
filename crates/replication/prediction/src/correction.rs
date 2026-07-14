//! Client-side visual correction for predicted components after rollback.
//!
//! Prediction rollback has two separate goals:
//! - the simulation must immediately use the corrected state produced by
//!   rollback and replay;
//! - the rendered value should not snap from the pre-rollback visual state to
//!   the corrected visual state in one frame.
//!
//! Correction is installed for components registered with
//! `add_correction`, `add_linear_correction`, or `add_correction_fn`. The
//! registration stores type-erased handlers in [`PredictionRegistry`] so the
//! post-rollback correction system can run the relevant frame-interpolation
//! rule and create [`VisualCorrection`] for any corrected component `C`.
//!
//! Normal frame interpolation works like this:
//! - [`FrameInterpolationSystems::Restore`] runs in `RunFixedMainLoop` before
//!   fixed simulation and restores the live component `C` from
//!   [`FrameInterpolationHistory`] so fixed systems read simulation state,
//!   not the previous frame's visual interpolation.
//! - [`FrameInterpolationSystems::Update`] runs in `FixedPostUpdate` and records
//!   the latest fixed value into [`FrameInterpolationHistory`]. This set is
//!   disabled during rollback.
//! - [`FrameInterpolationSystems::Interpolate`] runs in `PostUpdate` and writes
//!   the visual `C` by interpolating the history's previous/current values with
//!   the current fixed overstep.
//!
//! Rollback stores the pre-rollback visual value:
//! - rollback runs in `PreUpdate` and restores predicted components from
//!   [`PredictionHistory`] or confirmed history before replaying fixed ticks;
//! - just before a live predicted `C` is overwritten with the rollback value,
//!   rollback inserts [`PreviousVisual`] if `C` has correction enabled;
//! - replay advances the live component to the corrected simulation value for
//!   the current tick, but [`FrameInterpolationSystems::Update`] is skipped
//!   while rollback is active, so frame history must be repaired manually.
//!
//! Post-rollback processing repairs frame history before creating corrections:
//! - `repair_frame_interpolation_history` runs for every predicted component
//!   in [`RollbackSystems::EndRollback`]. It updates
//!   [`FrameInterpolationHistory`] from the corrected live `C` and the previous
//!   tick entry in [`PredictionHistory`].
//! - `update_frame_interpolation_post_rollback` then calls the selected
//!   frame-interpolation rule from [`InterpolationRegistry`] for archetypes
//!   that contain [`PreviousVisual`]. This temporarily writes the corrected
//!   visual sample into the live component, using the same component or bundle
//!   rule that normal frame interpolation would use.
//! - It compares that corrected visual sample with [`PreviousVisual`], inserts
//!   [`VisualCorrection`] with the resulting visual error, removes
//!   [`PreviousVisual`], and restores the live component back to the corrected
//!   simulation value from [`FrameInterpolationHistory`].
//!
//! Finally, `add_visual_correction` runs in
//! [`RollbackSystems::VisualCorrection`], ordered after
//! [`FrameInterpolationSystems::Interpolate`]. Normal frame interpolation first
//! writes the corrected visual value for the render frame; visual correction
//! then applies the decaying [`VisualCorrection`] error on top, using
//! [`PredictionManager::correction_policy`] and the correction function
//! registered for `C`. Once the error is small enough, [`VisualCorrection`]
//! is removed.

use crate::SyncComponent;
use crate::manager::PredictionManager;
use crate::predicted_history::PredictionHistory;
use crate::registry::PredictionRegistry;
use crate::rollback::RollbackSystems;
use alloc::vec::Vec;
use bevy_app::prelude::*;
use bevy_ecs::{
    archetype::{Archetype, ArchetypeGeneration, ArchetypeId, Archetypes},
    change_detection::Tick as ChangeTick,
    component::{ComponentId, Components, Mutable, StorageType},
    prelude::*,
    query::{FilteredAccess, FilteredAccessSet},
    system::{SystemMeta, SystemParam, SystemParamValidationError},
    world::unsafe_world_cell::UnsafeWorldCell,
};
use bevy_platform::collections::HashMap;
use bevy_reflect::Reflect;
use bevy_time::{Fixed, Time, Virtual};
use bevy_utils::prelude::DebugName;
use core::fmt::Debug;
use lightyear_core::ecs_utils::{table_component_slice, table_for_archetype};
use lightyear_core::prelude::*;
use lightyear_frame_interpolation::FrameInterpolationSystems;
use lightyear_interpolation::registry::InterpolationRegistry;
use lightyear_interpolation::rules::frame_interpolate::{
    CachedFrameInterpolationApply, FrameInterpolationContext,
};
use lightyear_interpolation::rules::{InterpolationRuleId, RuleKind};
use lightyear_replication::deferred_entity::DeferredEntityCommands;
use lightyear_replication::delta::Diffable;
use lightyear_replication::registry::{ComponentKind, LerpFn};
use tracing::trace;

/// The visual value of the component before the rollback started
#[derive(Component, Debug, Reflect)]
#[require(FrameInterpolate)]
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
    create_visual_correction: ErasedCreateVisualCorrectionFn,
    restore_history: ErasedRestorePostRollbackFrameHistoryFn,
    correction_fn: unsafe fn(),
    live_component_id: ComponentId,
    previous_visual_component_id: ComponentId,
    frame_history_component_id: ComponentId,
}

impl ErasedPostRollbackCorrection {
    pub(crate) fn new<C, D>(world: &mut World, correction_fn: LerpFn<D>) -> Self
    where
        C: SyncComponent + Diffable<D>,
        D: Debug + Send + Sync + 'static,
    {
        Self {
            kind: ComponentKind::of::<C>(),
            create_visual_correction: create_visual_correction_from_live_erased::<C, D>,
            restore_history: restore_frame_history_post_rollback_erased::<C, D>,
            correction_fn: unsafe { core::mem::transmute::<LerpFn<D>, unsafe fn()>(correction_fn) },
            live_component_id: world.register_component::<C>(),
            previous_visual_component_id: world.register_component::<PreviousVisual<C>>(),
            frame_history_component_id: world.register_component::<FrameInterpolationHistory<C>>(),
        }
    }

    pub(crate) fn kind(&self) -> ComponentKind {
        self.kind
    }

    fn add_access(&self, filtered_access: &mut FilteredAccess) {
        filtered_access.add_write(self.live_component_id);
        filtered_access.add_read(self.previous_visual_component_id);
        filtered_access.add_write(self.frame_history_component_id);
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

    pub(crate) fn apply_correction<D: Default>(&self, error: D, ratio: f32) -> D {
        let correction_fn =
            unsafe { core::mem::transmute::<unsafe fn(), LerpFn<D>>(self.correction_fn) };
        correction_fn(D::default(), error, ratio)
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

/// Cached correction-time frame interpolation rules selected for archetypes
/// that contain at least one [`PreviousVisual`] component.
///
/// Correction reuses the same interpolation rules as frame interpolation, but
/// it only needs to run them for components that were visually captured before
/// rollback. This cache avoids re-evaluating filters and bundle precedence for
/// every post-rollback correction pass.
#[derive(Debug)]
pub(crate) struct PostRollbackCorrectionArchetypes {
    generation: ArchetypeGeneration,
    rule_count: usize,
    correction_count: usize,
    archetypes: Vec<CachedPostRollbackCorrectionArchetype>,
}

impl Default for PostRollbackCorrectionArchetypes {
    fn default() -> Self {
        Self {
            generation: ArchetypeGeneration::initial(),
            rule_count: 0,
            correction_count: 0,
            archetypes: Vec::new(),
        }
    }
}

impl PostRollbackCorrectionArchetypes {
    fn clear(&mut self) {
        self.generation = ArchetypeGeneration::initial();
        self.archetypes.clear();
    }

    /// Refreshes the correction cache for newly-created archetypes.
    ///
    /// Rule selection mirrors frame interpolation, except rule members must all
    /// have an active `PreviousVisual<C>` correction marker on the archetype.
    /// A high-priority no-apply rule still claims its members, so it blocks
    /// lower-priority rules just like normal interpolation rule selection.
    fn update(
        &mut self,
        archetypes: &Archetypes,
        components: &Components,
        prediction_registry: &PredictionRegistry,
        interpolation_registry: &InterpolationRegistry,
    ) {
        let rule_count = interpolation_registry.rule_count();
        let correction_count = prediction_registry.post_rollback_corrections().count();
        if self.rule_count != rule_count || self.correction_count != correction_count {
            self.clear();
            self.rule_count = rule_count;
            self.correction_count = correction_count;
        }

        let old_generation = core::mem::replace(&mut self.generation, archetypes.generation());
        for archetype in archetypes[old_generation..].iter() {
            let mut cached = CachedPostRollbackCorrectionArchetype::new(archetype.id());
            cached.collect_active_members(archetype, prediction_registry);
            if cached.active_members.is_empty() {
                continue;
            }

            for kind in interpolation_registry.rule_kinds() {
                if let Some(rule_id) =
                    interpolation_registry.select_rule_for_archetype(components, archetype, kind)
                {
                    cached.selected_rules.insert(kind, rule_id);
                }
            }

            cached.resolve_apply_rules(interpolation_registry);
            cached.assert_all_members_covered();
            self.archetypes.push(cached);
        }
    }

    fn iter(&self) -> impl Iterator<Item = &CachedPostRollbackCorrectionArchetype> {
        self.archetypes.iter()
    }
}

/// Cached correction-time interpolation policy for one archetype.
#[derive(Debug)]
struct CachedPostRollbackCorrectionArchetype {
    id: ArchetypeId,
    active_members: Vec<ComponentKind>,
    covered_members: Vec<ComponentKind>,
    selected_rules: HashMap<RuleKind, InterpolationRuleId>,
    apply_rules: Vec<CachedFrameInterpolationApply>,
}

impl CachedPostRollbackCorrectionArchetype {
    fn new(id: ArchetypeId) -> Self {
        Self {
            id,
            active_members: Vec::new(),
            covered_members: Vec::new(),
            selected_rules: HashMap::default(),
            apply_rules: Vec::new(),
        }
    }

    fn id(&self) -> ArchetypeId {
        self.id
    }

    fn apply_rules(&self) -> &[CachedFrameInterpolationApply] {
        &self.apply_rules
    }

    fn collect_active_members(
        &mut self,
        archetype: &Archetype,
        prediction_registry: &PredictionRegistry,
    ) {
        self.active_members.extend(
            prediction_registry
                .post_rollback_corrections()
                .filter(|correction| archetype.contains(correction.previous_visual_component_id))
                .map(|correction| correction.kind()),
        );
    }

    fn resolve_apply_rules(&mut self, registry: &InterpolationRegistry) {
        self.apply_rules.clear();
        self.covered_members.clear();

        // `selected_rules` contains the best matching rule for each rule kind
        // on this archetype, for example `A`, `B`, and `(A, B)`. Correction
        // only cares about members that have `PreviousVisual<C>` on this
        // archetype. We walk selected rules by precedence and let each
        // applicable rule claim all of its active members so higher-priority
        // bundle rules can suppress overlapping single-component rules.
        let mut candidates = self
            .selected_rules
            .iter()
            .filter_map(|(&kind, &rule_id)| registry.rule(rule_id).map(|_| (kind, rule_id)))
            .collect::<Vec<_>>();
        candidates.sort_by(|(_, lhs), (_, rhs)| registry.cmp_rule_precedence(*lhs, *rhs));

        let mut claimed_members = Vec::new();
        for (_, rule_id) in candidates {
            let Some(rule) = registry.rule(rule_id) else {
                continue;
            };
            if rule
                .members()
                .iter()
                .any(|member| !self.active_members.contains(member))
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
            if let Some(apply) = registry.cached_frame_apply_component(rule_id) {
                self.covered_members.extend(rule.members().iter().copied());
                self.apply_rules.push(apply);
            }
        }
    }

    fn assert_all_members_covered(&self) {
        for member in &self.active_members {
            assert!(
                self.covered_members.contains(member),
                "No interpolation function was found for correction. Register an interpolation rule with an interpolation function for this component or bundle before calling add_correction/add_linear_correction/add_correction_fn."
            );
        }
    }
}

#[derive(Resource, Default)]
struct PostRollbackCorrectionSystemInstalled;

/// Installs built-in visual correction systems for predicted component `C`.
///
/// The post-rollback bridge runs in [`PreUpdate`], in
/// [`RollbackSystems::EndRollback`], and the visual error decay runs in
/// [`PostUpdate`], in [`RollbackSystems::VisualCorrection`]. Registration is
/// idempotent for the shared post-rollback system; each corrected component
/// still gets its own typed visual-correction decay system.
pub fn add_correction_systems<
    C: SyncComponent + Diffable<D>,
    D: Default + Clone + Debug + Send + Sync + 'static,
>(
    app: &mut App,
) {
    // When rollback finishes, compute the new corrected visual value and compare it
    // with the original visual value to set the visual correction error.
    if !app
        .world()
        .contains_resource::<PostRollbackCorrectionSystemInstalled>()
    {
        app.insert_resource(PostRollbackCorrectionSystemInstalled);
        app.add_systems(
            PreUpdate,
            update_frame_interpolation_post_rollback
                .in_set(RollbackSystems::EndRollback)
                .before(crate::rollback::end_rollback),
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

/// Creates visual corrections after a rollback from frame-interpolation state.
///
/// This system runs in [`PreUpdate`], in [`RollbackSystems::EndRollback`],
/// after [`repair_frame_interpolation_history`] and before
/// [`crate::rollback::end_rollback`]. It runs the selected frame-interpolation
/// rules to compute the corrected visual sample, creates [`VisualCorrection`]
/// from that sample and [`PreviousVisual`], then restores live components to
/// their corrected simulation values.
pub(crate) fn update_frame_interpolation_post_rollback(
    time: Res<Time<Fixed>>,
    timeline: Res<LocalTimeline>,
    prediction_registry: Res<PredictionRegistry>,
    interpolation_registry: Res<InterpolationRegistry>,
    correction_world: PostRollbackCorrectionWorld,
    mut correction_archetypes: Local<PostRollbackCorrectionArchetypes>,
    mut commands: Commands,
) {
    let ctx = PostRollbackCorrectionContext {
        // NOTE: this is the overstep from the previous frame since we are running this before RunFixedMainLoop
        overstep: time.overstep_fraction(),
        tick: timeline.tick(),
    };
    let mut deferred_apply = DeferredEntityCommands::default();
    let world = correction_world.world;

    correction_archetypes.update(
        world.archetypes(),
        world.components(),
        &prediction_registry,
        &interpolation_registry,
    );

    // 1. Reuse the interpolation rules selected for this archetype to compute
    // the corrected visual sample at the current fixed-overstep. This can run
    // bundle rules such as `(A, B)`, so correction sees the same visual state
    // that frame interpolation would have produced.
    apply_frame_interpolation_for_visual_correction(
        world,
        &correction_archetypes,
        &interpolation_registry,
        ctx,
        &mut deferred_apply,
    );

    // 2. Compare the original pre-rollback visual value against the corrected
    // visual sample to create `VisualCorrection<D>`, then restore the live
    // component to the corrected simulation value. The visual correction is
    // applied later in `RollbackSystems::VisualCorrection`.
    for correction in prediction_registry.post_rollback_corrections() {
        correction.create_visual_correction(world, ctx, &mut deferred_apply);
        correction.restore_history(world);
    }
    deferred_apply.apply(&mut commands);
}

/// Repair the frame-interpolation history of `C` to reflect `C`'s prediction
/// timeline.
///
/// If `C` was replayed due to rollback then it may have different values from
/// before the rollback. Its frame-interpolation history still holds onto those
/// old values and needs to be corrected.
pub(crate) fn repair_frame_interpolation_history<C: Component<Mutability = Mutable> + Clone>(
    timeline: Res<LocalTimeline>,
    mut components: Query<(
        Option<&C>,
        &PredictionHistory<C>,
        &mut FrameInterpolationHistory<C>,
    )>,
) {
    let tick = timeline.tick();
    for (component, prediction_history, mut frame_history) in &mut components {
        frame_history.previous_value = prediction_history.get(tick - 1).cloned();
        frame_history.current_value = component.cloned();
    }
}

/// Applies selected frame-interpolation rules to post-rollback visual samples.
///
/// This helper is called by [`update_frame_interpolation_post_rollback`] in
/// [`PreUpdate`], in [`RollbackSystems::EndRollback`], after frame histories
/// have been repaired and before [`VisualCorrection`] is created. It iterates
/// the correction archetype cache and runs each selected type-erased frame
/// apply function, so bundle rules and component rules use the same precedence
/// as normal frame interpolation.
fn apply_frame_interpolation_for_visual_correction(
    world: UnsafeWorldCell,
    correction_archetypes: &PostRollbackCorrectionArchetypes,
    interpolation_registry: &InterpolationRegistry,
    ctx: PostRollbackCorrectionContext,
    deferred_apply: &mut DeferredEntityCommands,
) {
    for cached_archetype in correction_archetypes.iter() {
        let Some(archetype) = world.archetypes().get(cached_archetype.id()) else {
            continue;
        };
        for apply in cached_archetype.apply_rules() {
            (apply.apply_frame_interpolation())(
                world,
                archetype,
                interpolation_registry,
                apply.rule_id(),
                FrameInterpolationContext {
                    overstep: ctx.overstep,
                },
                false,
                deferred_apply,
            );
        }
    }
}

/// Creates a `VisualCorrection<D>` from the corrected visual sample.
///
/// This erased handler is called by
/// [`update_frame_interpolation_post_rollback`] in [`PreUpdate`], in
/// [`RollbackSystems::EndRollback`], after frame-interpolation rules have
/// temporarily written the corrected visual value into live `C`. It compares
/// that value with [`PreviousVisual<C>`], stores the resulting diff in
/// [`VisualCorrection<D>`], and removes [`PreviousVisual<C>`].
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
        debug_assert_eq!(
            archetype.get_storage_type(frame_history_id),
            Some(StorageType::Table)
        );
        debug_assert_eq!(
            archetype.get_storage_type(previous_visual_id),
            Some(StorageType::Table)
        );
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

/// Restores live `C` to the corrected simulation value after sampling visuals.
///
/// This erased handler is called by
/// [`update_frame_interpolation_post_rollback`] in [`PreUpdate`], in
/// [`RollbackSystems::EndRollback`], after [`VisualCorrection`] has been
/// created. The frame-apply phase temporarily writes visual values into live
/// components; this restores each live `C` from
/// `FrameInterpolationHistory<C>::current_value` so fixed simulation state
/// remains authoritative.
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
        debug_assert_eq!(
            archetype.get_storage_type(frame_history_id),
            Some(StorageType::Table)
        );
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

/// Applies and decays a stored visual correction after frame interpolation.
///
/// This typed system runs in [`PostUpdate`], in
/// [`RollbackSystems::VisualCorrection`], after
/// [`FrameInterpolationSystems::Interpolate`]. Frame interpolation first writes
/// the corrected visual value for the render frame; this system then applies
/// the decaying [`VisualCorrection`] error on top. If the remaining error is
/// small enough, it removes the correction component.
///
/// `C` must have an interpolation rule with a frame-interpolation apply
/// function, because correction uses that rule to sample the current visual
/// value right after rollback. The resulting rollback error is stored as `D`
/// and decayed by the correction function registered through
/// `add_correction`, `add_linear_correction`, or `add_correction_fn`.
pub(crate) fn add_visual_correction<
    C: SyncComponent + Diffable<D>,
    D: Default + Clone + Debug + Send + Sync + 'static,
>(
    time: Res<Time<Virtual>>,
    prediction: Res<PredictionRegistry>,
    manager: Single<&PredictionManager>,
    mut query: Query<(Entity, &mut C, &mut VisualCorrection<D>)>,
    mut commands: Commands,
) {
    let r = manager.correction_policy.lerp_ratio(time.delta());
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
            let new_error = prediction
                .apply_correction::<C, D>(previous_error.clone(), r)
                .expect("No correction function was found. Call add_correction, add_linear_correction, or add_correction_fn for this component.");
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
    use core::time::Duration;

    use bevy_ecs::system::RunSystemOnce;
    use bevy_math::{
        Curve,
        curve::{Ease, FunctionCurve, Interval},
    };
    use bevy_replicon::prelude::*;
    use bevy_state::app::StatesPlugin;
    use lightyear_interpolation::{
        registry::{AppInterpolationExt, InterpolationRegistry},
        rules::InterpolationFns,
    };
    use lightyear_replication::checkpoint::ReplicationCheckpointMap;
    use lightyear_replication::delta::Diffable as LightyearDiffable;
    use lightyear_replication::prelude::*;

    use super::*;
    use crate::registry::{PredictionBuilderExt, PredictionRegistry};

    #[derive(Component, Clone, Debug, Default, PartialEq)]
    struct CorrectionA(f32);

    #[derive(Component, Clone, Debug, Default, PartialEq)]
    struct CorrectionB(f32);

    #[derive(Component, Clone, Debug, PartialEq)]
    #[component(storage = "SparseSet")]
    struct SparseCorrection(f32);

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

    impl LightyearDiffable<CorrectionA> for CorrectionA {
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

    impl LightyearDiffable<CorrectionB> for CorrectionB {
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

    #[test]
    fn correction_registration_adds_frame_interpolation_setup() {
        let mut app = App::new();
        app.add_plugins((
            StatesPlugin,
            RepliconSharedPlugin {
                auth_method: AuthMethod::None,
            },
        ));
        app.init_resource::<PredictionRegistry>();

        app.component::<CorrectionA>().predict().add_correction();
        app.interpolate_with::<CorrectionA>(InterpolationFns::no_history(|start, end, t| {
            CorrectionA(start.0 + (end.0 - start.0) * t)
        }));

        assert!(app.is_plugin_added::<lightyear_frame_interpolation::FrameInterpolationPlugin>());
        app.finish();

        let entity = app
            .world_mut()
            .spawn((CorrectionA(1.0), PreviousVisual(CorrectionA(2.0))))
            .id();
        app.world_mut().flush();
        assert!(app.world().get::<FrameInterpolate>(entity).is_some());
        assert!(
            app.world()
                .get::<FrameInterpolationHistory<CorrectionA>>(entity)
                .is_some()
        );
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

    // Verifies that repair handles both surviving and removed components
    // without visual-correction metadata.
    #[test]
    fn repairs_frame_history_without_visual_correction() {
        const PREVIOUS_TICK: Tick = Tick(9);
        const CURRENT_TICK: Tick = Tick(10);

        // Corrected values produced by replay for the entity that retains its
        // `CorrectionA` component.
        const PREVIOUS_VALUE: f32 = 4.0;
        const CURRENT_VALUE: f32 = 10.0;

        // Corrected previous value for the entity whose replay removes its
        // `CorrectionA` component.
        const REMOVED_PREVIOUS_VALUE: f32 = 3.0;

        // Stale frame-history values from the discarded prediction timeline.
        const STALE_PREVIOUS_VALUE: f32 = 100.0;
        const STALE_CURRENT_VALUE: f32 = 200.0;
        const STALE_REMOVED_PREVIOUS_VALUE: f32 = 300.0;
        const STALE_REMOVED_CURRENT_VALUE: f32 = 400.0;

        let mut app = App::new();
        app.add_plugins((
            StatesPlugin,
            RepliconSharedPlugin {
                auth_method: AuthMethod::None,
            },
        ));
        app.init_resource::<PredictionRegistry>();
        app.init_resource::<InterpolationRegistry>();
        app.insert_resource(Time::<Fixed>::from_duration(Duration::from_secs(1)));
        app.insert_resource(LocalTimeline::default());
        app.world_mut()
            .resource_mut::<LocalTimeline>()
            .apply_delta(CURRENT_TICK.0 as i32);

        // Do not register with visual correction. Frame-history repair must work
        // without it.
        app.component::<CorrectionA>().predict();

        // Replay leaves the surviving component live at the current tick, while
        // prediction history contains its corrected previous-tick sample.
        let mut live_prediction = PredictionHistory::<CorrectionA>::default();
        live_prediction.add_predicted(PREVIOUS_TICK, Some(CorrectionA(PREVIOUS_VALUE)));
        let live = app
            .world_mut()
            .spawn((
                CorrectionA(CURRENT_VALUE),
                live_prediction,
                FrameInterpolationHistory::<CorrectionA> {
                    previous_value: Some(CorrectionA(STALE_PREVIOUS_VALUE)),
                    current_value: Some(CorrectionA(STALE_CURRENT_VALUE)),
                },
            ))
            .id();

        // Replay removed the component at CURRENT_TICK. Prediction history still
        // records REMOVED_PREVIOUS_VALUE at PREVIOUS_TICK.
        let mut removed_prediction = PredictionHistory::<CorrectionA>::default();
        removed_prediction.add_predicted(PREVIOUS_TICK, Some(CorrectionA(REMOVED_PREVIOUS_VALUE)));
        removed_prediction.add_predicted(CURRENT_TICK, None);
        let removed = app
            .world_mut()
            .spawn((
                removed_prediction,
                FrameInterpolationHistory::<CorrectionA> {
                    previous_value: Some(CorrectionA(STALE_REMOVED_PREVIOUS_VALUE)),
                    current_value: Some(CorrectionA(STALE_REMOVED_CURRENT_VALUE)),
                },
            ))
            .id();

        app.world_mut()
            .run_system_once(repair_frame_interpolation_history::<CorrectionA>)
            .unwrap();

        // A surviving component uses its previous sample from prediction
        // history and its current sample from the replayed live component.
        let live_history = app
            .world()
            .get::<FrameInterpolationHistory<CorrectionA>>(live)
            .unwrap();
        assert_eq!(
            live_history.previous_value,
            Some(CorrectionA(PREVIOUS_VALUE))
        );
        assert_eq!(live_history.current_value, Some(CorrectionA(CURRENT_VALUE)));

        // A removed component retains the previous prediction sample and clears
        // the current frame sample so interpolation cannot reinsert it.
        let removed_history = app
            .world()
            .get::<FrameInterpolationHistory<CorrectionA>>(removed)
            .unwrap();
        assert_eq!(
            removed_history.previous_value,
            Some(CorrectionA(REMOVED_PREVIOUS_VALUE))
        );
        assert_eq!(removed_history.current_value, None);
    }

    // Verifies that `.predict()` schedules frame-history repair after replay and
    // before rollback ends, even when visual correction is not registered.
    #[test]
    fn prediction_registration_schedules_frame_history_repair() {
        const PREVIOUS_TICK: Tick = Tick(9);
        const CURRENT_TICK: Tick = Tick(10);
        const PREVIOUS_VALUE: f32 = 4.0;
        const CURRENT_VALUE: f32 = 10.0;
        const STALE_PREVIOUS_VALUE: f32 = 100.0;
        const STALE_CURRENT_VALUE: f32 = 200.0;

        let replay =
            |mut components: Query<(&mut CorrectionA, &mut PredictionHistory<CorrectionA>)>| {
                for (mut component, mut history) in &mut components {
                    *component = CorrectionA(CURRENT_VALUE);
                    history.add_predicted(CURRENT_TICK, Some(component.clone()));
                }
            };

        let mut app = App::new();
        app.add_plugins((
            StatesPlugin,
            RepliconSharedPlugin {
                auth_method: AuthMethod::None,
            },
        ));
        app.configure_sets(
            PreUpdate,
            (
                RollbackSystems::Prepare.run_if(is_in_rollback),
                RollbackSystems::Rollback.run_if(is_in_rollback),
                RollbackSystems::EndRollback.run_if(is_in_rollback),
            )
                .chain(),
        );
        app.add_systems(PreUpdate, replay.in_set(RollbackSystems::Rollback));
        app.init_resource::<PredictionRegistry>();
        app.insert_resource(ReplicationCheckpointMap::default());
        app.insert_resource(LocalTimeline::default());
        app.world_mut()
            .resource_mut::<LocalTimeline>()
            .apply_delta(CURRENT_TICK.0 as i32);

        let prediction_manager = PredictionManager::default();
        prediction_manager.set_rollback_tick(PREVIOUS_TICK);
        app.world_mut()
            .spawn((prediction_manager, Rollback::FromInputs));

        app.component::<CorrectionA>().predict();
        app.finish();

        let mut prediction = PredictionHistory::<CorrectionA>::default();
        prediction.add_predicted(PREVIOUS_TICK, Some(CorrectionA(PREVIOUS_VALUE)));
        prediction.add_predicted(CURRENT_TICK, Some(CorrectionA(CURRENT_VALUE)));
        let entity = app
            .world_mut()
            .spawn((
                CorrectionA(CURRENT_VALUE),
                prediction,
                FrameInterpolationHistory::<CorrectionA> {
                    previous_value: Some(CorrectionA(STALE_PREVIOUS_VALUE)),
                    current_value: Some(CorrectionA(STALE_CURRENT_VALUE)),
                },
            ))
            .id();

        app.world_mut().run_schedule(PreUpdate);

        let frame_history = app
            .world()
            .get::<FrameInterpolationHistory<CorrectionA>>(entity)
            .unwrap();
        assert_eq!(
            frame_history.previous_value,
            Some(CorrectionA(PREVIOUS_VALUE))
        );
        assert_eq!(
            frame_history.current_value,
            Some(CorrectionA(CURRENT_VALUE))
        );
    }

    // Verifies that repair reads a sparse-set live component while updating
    // its table-stored prediction and frame histories.
    #[test]
    fn repairs_sparse_set_frame_history() {
        const PREVIOUS_TICK: Tick = Tick(9);
        const CURRENT_TICK: Tick = Tick(10);

        // Corrected values produced by replay for `SparseCorrection`.
        const PREVIOUS_VALUE: f32 = 4.0;
        const CURRENT_VALUE: f32 = 10.0;

        // Stale `SparseCorrection` frame-history values from the discarded
        // prediction timeline.
        const STALE_PREVIOUS_VALUE: f32 = 100.0;
        const STALE_CURRENT_VALUE: f32 = 200.0;

        let mut app = App::new();
        app.insert_resource(LocalTimeline::default());
        app.world_mut()
            .resource_mut::<LocalTimeline>()
            .apply_delta(CURRENT_TICK.0 as i32);

        // SparseCorrection stores the live component in a sparse set while both
        // history components use table storage. The stale sentinels verify that
        // repair reads the live value through Bevy's storage-independent query.
        let mut prediction = PredictionHistory::<SparseCorrection>::default();
        prediction.add_predicted(PREVIOUS_TICK, Some(SparseCorrection(PREVIOUS_VALUE)));
        let entity = app
            .world_mut()
            .spawn((
                SparseCorrection(CURRENT_VALUE),
                prediction,
                FrameInterpolationHistory::<SparseCorrection> {
                    previous_value: Some(SparseCorrection(STALE_PREVIOUS_VALUE)),
                    current_value: Some(SparseCorrection(STALE_CURRENT_VALUE)),
                },
            ))
            .id();

        app.world_mut()
            .run_system_once(repair_frame_interpolation_history::<SparseCorrection>)
            .unwrap();

        // The sparse live value supplies the current sample, and prediction
        // history supplies the previous sample exactly as in the table case.
        let frame_history = app
            .world()
            .get::<FrameInterpolationHistory<SparseCorrection>>(entity)
            .unwrap();
        assert_eq!(
            frame_history.previous_value,
            Some(SparseCorrection(PREVIOUS_VALUE))
        );
        assert_eq!(
            frame_history.current_value,
            Some(SparseCorrection(CURRENT_VALUE))
        );
    }

    // Verifies that visual correction rejects a component without an
    // interpolation rule because it cannot compute the corrected visual sample.
    #[test]
    #[should_panic(expected = "No interpolation function was found for correction")]
    fn post_rollback_correction_requires_interpolation_rule() {
        let mut app = App::new();
        app.add_plugins((
            StatesPlugin,
            RepliconSharedPlugin {
                auth_method: AuthMethod::None,
            },
        ));
        app.init_resource::<PredictionRegistry>();
        app.init_resource::<InterpolationRegistry>();
        app.insert_resource(Time::<Fixed>::from_duration(Duration::from_secs(1)));
        app.insert_resource(LocalTimeline::default());
        app.world_mut()
            .resource_mut::<LocalTimeline>()
            .apply_delta(10);

        app.component::<CorrectionA>().predict().add_correction();

        let mut history = PredictionHistory::<CorrectionA>::default();
        history.add_predicted(Tick(9), Some(CorrectionA(4.0)));

        app.world_mut().spawn((
            CorrectionA(10.0),
            PreviousVisual(CorrectionA(12.0)),
            history,
            FrameInterpolationHistory::<CorrectionA>::default(),
        ));

        app.world_mut()
            .run_system_once(update_frame_interpolation_post_rollback)
            .unwrap();
    }

    // Verifies that post-rollback correction samples the selected bundle rule,
    // creates each correction error, and restores both live component values.
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

        app.component::<CorrectionA>().predict().add_correction();
        app.component::<CorrectionB>().predict().add_correction();

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
            .run_system_once(repair_frame_interpolation_history::<CorrectionA>)
            .unwrap();
        app.world_mut()
            .run_system_once(repair_frame_interpolation_history::<CorrectionB>)
            .unwrap();
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
