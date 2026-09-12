//! Client-side visual correction for predicted components.
//!
//! Prediction rollback has two separate goals:
//! - the simulation must immediately use the corrected state produced by
//!   rollback and replay;
//! - the rendered value should not snap from the pre-rollback visual state to
//!   the corrected visual state in one frame.
//!
//! Correction is installed for components registered with `add_correction`,
//! `add_linear_correction`, or `add_correction_fn`. The registration stores
//! type-erased handlers in [`PredictionRegistry`] so one system can record a
//! [`VisualCorrection`] for any corrected component `C`.
//!
//! # How a correction is recorded
//!
//! Every correction is one measurement of one jump: the value that was on screen
//! before it, against the value that is on screen now.
//!
//! ```text
//! rendered = live + error        (the apply adds the error every frame)
//! error    = previous - live     (recorded once, per jump)
//! ```
//!
//! The value before the jump is saved as [`PreviousVisual`], and the value now is
//! whatever the live component holds when the correction is recorded. Recording
//! happens in `PostUpdate`, after [`FrameInterpolationSystems::Interpolate`] has
//! written the value the frame renders, so the measurement is against exactly
//! what the entity would show without a correction. Nothing needs to know how
//! that value was produced: the frame-interpolation rules, including bundle
//! rules and the fixed overstep, have already run.
//!
//! That is also why there is no correction-time sampling of its own. Correction
//! used to re-run the frame-interpolation rules at `EndRollback` to synthesize
//! the value frame interpolation was about to write; now it simply runs after the
//! real thing. A timeline switch is the same measurement with a different value
//! before the jump (see [`crate::switch`]), so it goes through this same system.
//!
//! # How the error is given up
//!
//! [`VisualCorrection`] holds the error and the clock reading it was recorded at.
//! `update_visual_correction` multiplies the error by a keep ratio each frame:
//!
//! - on the entity's [`CorrectionPolicy`], or the global one on
//!   [`PredictionManager`], which defaults to an unbounded exponential and can
//!   be set to any [`CorrectionEase`] with its own duration;
//! - on a live [`SwitchBlend`] window's curve instead, when the entity has one.
//!
//! The two combine by taking whichever gives the error up more slowly at that
//! instant, so a blend is never shortened by the policy and a jump arriving late
//! in a window is never dumped by the curve's tail. A correction that has
//! converged is dropped rather than carried.
//!
//! # The rest of the frame
//!
//! - [`FrameInterpolationSystems::Restore`] runs in `RunFixedMainLoop` before
//!   fixed simulation and restores the live component `C` from
//!   [`FrameInterpolationHistory`] so fixed systems read simulation state, not
//!   the previous frame's visual interpolation.
//! - [`FrameInterpolationSystems::Update`] runs in `FixedPostUpdate` and records
//!   the latest fixed value into [`FrameInterpolationHistory`]. This set is
//!   disabled during rollback.
//! - [`FrameInterpolationSystems::Interpolate`] runs in `PostUpdate` and writes
//!   the visual `C` by interpolating the history's previous/current values with
//!   the current fixed overstep.
//! - Rollback runs in `PreUpdate`. Just before a live predicted `C` is
//!   overwritten, rollback saves [`PreviousVisual`] if `C` has correction
//!   enabled, whatever it is about to do to the component. Replay then advances
//!   the live component to the corrected simulation value for the current tick,
//!   but [`FrameInterpolationSystems::Update`] is skipped while rollback is
//!   active, so history must be repaired manually:
//!   `update_frame_interpolation_post_rollback` runs for every predicted
//!   component in [`RollbackSystems::EndRollback`] and updates
//!   [`FrameInterpolationHistory`] from the corrected live `C` and the previous
//!   tick entry in [`PredictionHistory`].
//!
//! [`PreviousVisual`] is only removed by the system that records the correction,
//! so a component that is removed and added back still has its jump measured, and
//! a component that never comes back does not leave its saved value behind.
use crate::SyncComponent;
use crate::archetypes::{CachedPredictionComponent, UpdateFrameInterpolationPostRollbackWorld};
use crate::manager::PredictionManager;
use crate::predicted_history::PredictionHistory;
use crate::registry::PredictionRegistry;
use crate::rollback::RollbackSystems;
use crate::switch::{SwitchBlend, SwitchDirection};
use alloc::vec::Vec;
use bevy_app::prelude::*;
use bevy_ecs::{
    archetype::{Archetype, ArchetypeGeneration, ArchetypeId, Archetypes},
    change_detection::Tick as ChangeTick,
    component::{ComponentId, Mutable},
    prelude::*,
    query::{FilteredAccess, FilteredAccessSet},
    system::{SystemMeta, SystemParam, SystemParamValidationError},
    world::unsafe_world_cell::UnsafeWorldCell,
};
use bevy_reflect::Reflect;
use bevy_time::{Time, Virtual};
use bevy_utils::prelude::DebugName;
use core::fmt::Debug;
use lightyear_core::ecs_utils::write_component_with_change_detection;
use lightyear_core::prelude::*;
use lightyear_frame_interpolation::FrameInterpolationSystems;
use lightyear_replication::deferred_entity::DeferredEntityCommands;
use lightyear_replication::diffable::Diffable;
use lightyear_replication::registry::{ComponentRegistry, LerpFn};
use lightyear_utils::ecs::get_component_unchecked;
use tracing::trace;

/// The visual value of the component before the rollback started
#[derive(Component, Debug, Reflect)]
#[require(FrameInterpolate)]
pub struct PreviousVisual<C: Component>(pub C);

#[derive(Component, Debug, Reflect)]
pub struct VisualCorrection<D> {
    /// The error between the original visual value and the new visual value.
    /// Will decay over time.
    ///
    /// It is an offset: the apply adds it to the live value every frame, so what
    /// is rendered is `live + error`, and the decay shrinks it towards zero.
    ///
    /// A live [`SwitchBlend`] window shapes that decay instead of the entity's
    /// [`CorrectionPolicy`].
    pub error: D,
    /// The visual-clock reading this error was recorded at, in seconds.
    ///
    /// A bounded curve needs to know how far into its window it is, so it is
    /// measured from here. The exponential does not (its ratio is constant), but
    /// it is recorded for every correction so the apply has one shape to read.
    pub(crate) start_secs: f32,
}

impl<D> VisualCorrection<D> {
    /// A correction decaying on the entity's [`CorrectionPolicy`], starting now.
    pub fn new(error: D, start_secs: f32) -> Self {
        Self { error, start_secs }
    }

    /// How long this error has been decaying at the `now_secs` clock reading.
    pub fn elapsed_secs(&self, now_secs: f32) -> f32 {
        (now_secs - self.start_secs).max(0.0)
    }
}

/// Type-erased record of the correction for a saved value, per corrected
/// component.
pub(crate) type ErasedCreateVisualCorrectionFn =
    fn(UnsafeWorldCell, &Archetype, f32, &mut DeferredEntityCommands);

/// Type-erased save of the value a timeline switch blends from, for one
/// corrected type.
///
/// See [`save_previous_visual_for_switch_erased`] for what each direction does.
pub(crate) type ErasedSavePreviousVisualForSwitchFn =
    fn(UnsafeWorldCell, Entity, SwitchDirection, &mut DeferredEntityCommands);

/// Type-erased cleanup of the state a switch keeps for one corrected type.
///
/// Used when a blend window ends: the value the blend started from, and the
/// error it was writing, have no reader left.
pub(crate) type ErasedRemoveSwitchStateFn = fn(Entity, &mut DeferredEntityCommands);

/// Type-erased post-rollback correction metadata registered for one component.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ErasedPostRollbackCorrection {
    create_visual_correction: ErasedCreateVisualCorrectionFn,
    correction_fn: unsafe fn(),
    live_component_id: ComponentId,
    previous_visual_component_id: ComponentId,
    frame_history_component_id: ComponentId,
    /// The correction column the switch systems write.
    visual_correction_component_id: ComponentId,
    /// Presence of the entity-level blend marker: a window waiting for a
    /// component it will blend keeps the saved value until it arrives.
    switch_blend_component_id: ComponentId,
    save_previous_visual_for_switch: ErasedSavePreviousVisualForSwitchFn,
    remove_switch_state: ErasedRemoveSwitchStateFn,
}

impl ErasedPostRollbackCorrection {
    pub(crate) fn new<C, D>(world: &mut World, correction_fn: LerpFn<D>) -> Self
    where
        C: SyncComponent + Diffable<D>,
        D: Default + Debug + Send + Sync + 'static,
    {
        Self {
            create_visual_correction: create_visual_correction_erased::<C, D>,
            correction_fn: unsafe { core::mem::transmute::<LerpFn<D>, unsafe fn()>(correction_fn) },
            live_component_id: world.register_component::<C>(),
            previous_visual_component_id: world.register_component::<PreviousVisual<C>>(),
            frame_history_component_id: world.register_component::<FrameInterpolationHistory<C>>(),
            visual_correction_component_id: world.register_component::<VisualCorrection<D>>(),
            switch_blend_component_id: world.register_component::<SwitchBlend>(),
            save_previous_visual_for_switch: save_previous_visual_for_switch_erased::<C, D>,
            remove_switch_state: remove_switch_state_erased::<C, D>,
        }
    }

    /// Returns the save handler as a freestanding function pointer.
    ///
    /// See [`ErasedSavePreviousVisualForSwitchFn`] for the exact contract. The
    /// pointer is `Copy`, so switch systems can collect the handlers they need
    /// once per run and release the registry borrow before working.
    pub(crate) fn save_previous_visual_for_switch_fn(&self) -> ErasedSavePreviousVisualForSwitchFn {
        self.save_previous_visual_for_switch
    }

    /// Returns the switch-state cleanup handler as a freestanding function
    /// pointer.
    pub(crate) fn remove_switch_state_fn(&self) -> ErasedRemoveSwitchStateFn {
        self.remove_switch_state
    }

    /// Declares the accesses used by [`CorrectionWorld`].
    ///
    /// The systems that take it read the value on screen, the saved value a
    /// correction is measured from, and the blend marker, and write the
    /// correction and the frame history; every structural change is queued
    /// through [`DeferredEntityCommands`]. Declaring exactly those accesses keeps
    /// them off the exclusive path their `&mut World` predecessors required.
    pub(crate) fn add_correction_access(&self, filtered_access: &mut FilteredAccess) {
        filtered_access.add_read(self.live_component_id);
        filtered_access.add_read(self.previous_visual_component_id);
        filtered_access.add_read(self.switch_blend_component_id);
        filtered_access.add_write(self.visual_correction_component_id);
        filtered_access.add_write(self.frame_history_component_id);
    }

    /// Records this type's corrections for every entity with a saved value.
    ///
    /// See [`create_visual_correction_erased`] for the contract.
    pub(crate) fn create_visual_correction(
        &self,
        world: UnsafeWorldCell,
        archetype: &Archetype,
        start_secs: f32,
        deferred: &mut DeferredEntityCommands,
    ) {
        (self.create_visual_correction)(world, archetype, start_secs, deferred);
    }

    pub(crate) fn update_correction<D: Default>(&self, error: D, ratio: f32) -> D {
        let correction_fn =
            unsafe { core::mem::transmute::<unsafe fn(), LerpFn<D>>(self.correction_fn) };
        correction_fn(D::default(), error, ratio)
    }
}

/// System param exposing a low-level world cell for the correction systems.
///
/// Access is declared from the erased correction handlers registered in
/// [`PredictionRegistry`], so the dispatcher works without taking `&mut World`.
pub(crate) struct CorrectionWorld<'w> {
    world: UnsafeWorldCell<'w>,
}

impl<'w> CorrectionWorld<'w> {
    /// The world cell. Callers must respect the accesses declared in
    /// [`Self::init_access`].
    pub(crate) fn world(&self) -> UnsafeWorldCell<'w> {
        self.world
    }
}

unsafe impl SystemParam for CorrectionWorld<'_> {
    type State = ();
    type Item<'world, 'state> = CorrectionWorld<'world>;

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
                correction.add_correction_access(&mut filtered_access);
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
        Ok(CorrectionWorld { world })
    }
}

/// Marks that the shared correction creation system is installed.
#[derive(Resource)]
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
    // One shared system records the corrections, after frame interpolation has
    // written the value this frame renders and before the apply adds them.
    if !app
        .world()
        .contains_resource::<PostRollbackCorrectionSystemInstalled>()
    {
        app.insert_resource(PostRollbackCorrectionSystemInstalled);
        app.add_systems(
            PostUpdate,
            create_visual_corrections
                // The measurement is `previous - live`, so `live` has to be the
                // value this frame renders. Without this edge the system is only
                // ordered before the apply, leaving it free to run *before*
                // frame interpolation, which would measure against the raw
                // replayed value instead — and the apply would then put that
                // offset on top of the interpolated value.
                .after(FrameInterpolationSystems::Interpolate)
                .before(RollbackSystems::VisualCorrection),
        );
    }
    app.configure_sets(
        PostUpdate,
        (
            FrameInterpolationSystems::Interpolate,
            // The apply must see the interpolated value: the recorded error is
            // the gap to the value on screen, so it is added to that value.
            RollbackSystems::VisualCorrection,
        )
            .chain(),
    );
    app.add_systems(
        PostUpdate,
        update_visual_correction::<C, D>.in_set(RollbackSystems::VisualCorrection),
    );
    // Correction state cannot outlive its component: without live `C` the
    // apply query never runs, so leftovers would linger until despawn — and a
    // lingering blend window would permanently block every later switch.
    app.add_observer(remove_correction_state_on_live_removed::<C, D>);
}

/// Drops the decaying correction when its live component is removed.
///
/// Without live `C` nothing applies or decays the error, so it would sit there
/// until despawn and be added to whatever the value becomes when the component
/// returns.
///
/// The saved value is deliberately kept: a component can be removed and added
/// back — replication flux, or a rollback that removes it at the rollback tick
/// and re-adds it later in the replay — and the correction for the jump that
/// causes is measured from it. The system that computes corrections decides when
/// a saved value is past saving; see [`crate::correction`]'s creation system.
fn remove_correction_state_on_live_removed<C: Component, D: Send + Sync + 'static>(
    trigger: On<Remove, C>,
    mut commands: Commands,
) {
    commands
        .entity(trigger.entity)
        .remove::<VisualCorrection<D>>();
}

/// before the rollback. Its frame-interpolation history still holds onto those
/// old values and needs to be corrected.
pub(crate) fn update_frame_interpolation_post_rollback(
    mut prediction_world: UpdateFrameInterpolationPostRollbackWorld,
    timeline: Option<Res<LocalTimeline>>,
    prediction_registry: Res<PredictionRegistry>,
    component_registry: Res<ComponentRegistry>,
) {
    let Some(timeline) = timeline else {
        return;
    };
    let tick = timeline.tick();
    prediction_world.update_archetypes(&prediction_registry, &component_registry);

    let world = prediction_world.world;
    for (archetype, cached) in prediction_world.predicted_archetypes() {
        if !cached.default_query_target {
            continue;
        }
        for component in &cached.predicted_components {
            if component.prediction_history_storage.is_none()
                || component.frame_interpolation_history_storage.is_none()
            {
                continue;
            }
            // SAFETY: the cache records the exact live, prediction-history, and frame-history
            // component ids for this archetype, and the system declares their required accesses.
            unsafe {
                (component.update_frame_interpolation_post_rollback)(
                    world, archetype, component, tick,
                );
            }
        }
    }
}

/// Updates one cached component's frame history after rollback replay.
///
/// # Safety
///
/// `component` must describe `C`, `PredictionHistory<C>`, and
/// `FrameInterpolationHistory<C>` for `archetype`, and the caller must hold the accesses declared
/// by [`UpdateFrameInterpolationPostRollbackWorld`].
pub(crate) unsafe fn update_frame_interpolation_post_rollback_component<
    C: Component<Mutability = Mutable> + Clone,
>(
    world: UnsafeWorldCell,
    archetype: &Archetype,
    component: &CachedPredictionComponent,
    tick: Tick,
) {
    let prediction_history_storage = component
        .prediction_history_storage
        .expect("frame-history repair requires prediction history");
    for entity in archetype.entities() {
        let prediction_history = unsafe {
            get_component_unchecked(
                world,
                entity,
                archetype.table_id(),
                prediction_history_storage,
                component.prediction_history_id,
            )
            .deref::<PredictionHistory<C>>()
        };
        let current_value = component.component_storage.map(|storage| unsafe {
            get_component_unchecked(
                world,
                entity,
                archetype.table_id(),
                storage,
                component.component_id,
            )
            .deref::<C>()
            .clone()
        });
        let repaired = FrameInterpolationHistory::<C> {
            previous_value: prediction_history.get(tick - 1).cloned(),
            current_value,
        };

        // SAFETY: the dispatcher declares unique access to FrameInterpolationHistory<C>, and no
        // reference to that component is held by this callback.
        let written = unsafe {
            write_component_with_change_detection::<FrameInterpolationHistory<C>>(
                world,
                entity.id(),
                repaired,
            )
        };
        debug_assert!(written, "cached frame-history component should still exist");
    }
}

/// Saves the value a timeline switch blends from, for one correction type.
///
/// See [`ErasedSavePreviousVisualForSwitchFn`] for the contract. A switch to the
/// prediction timeline saves nothing: the forced rollback that it asks for
/// captures the same value from the same live component in `PreUpdate` of the
/// next frame, and nothing writes the live value in between. All this pass does
/// for that direction is drop the stale
/// [`FrameInterpolationHistory<C>`](lightyear_core::prelude::FrameInterpolationHistory),
/// which frame interpolation stopped updating while the entity was interpolated.
fn save_previous_visual_for_switch_erased<C, D>(
    world: UnsafeWorldCell,
    entity: Entity,
    direction: SwitchDirection,
    deferred: &mut DeferredEntityCommands,
) where
    C: SyncComponent + Diffable<D>,
    D: Default + Send + Sync + 'static,
{
    let Ok(entity_cell) = world.get_entity(entity) else {
        return;
    };
    // The history belongs to the era the entity is leaving. Frame interpolation
    // only records it for archetypes that carry `FrameInterpolate`, so while the
    // entity was interpolated this history was frozen at the value from before
    // that; and once the entity becomes interpolated, frame interpolation stops
    // reading it. Either way it does not describe where the entity is going, and
    // leaving it would let the next era restore a stale value over the live one.
    // Removing it lets the next era seed a fresh one from the live value.
    deferred.remove::<FrameInterpolationHistory<C>>(entity);
    if direction == SwitchDirection::ToPredicted {
        // The rollback this switch asks for is what captures the value to blend
        // from: it takes the live value in `PreUpdate` of the next frame, and
        // nothing writes the live value between here and there, so it captures
        // this same value. Saving it here would write the same thing twice.
        return;
    }
    // SAFETY: the caller declares read access to live `C`.
    let Some(value) = (unsafe { entity_cell.get::<C>() }).cloned() else {
        return;
    };
    trace!(
        target: "lightyear_debug::prediction",
        kind = "switch_previous_visual_saved",
        schedule = "PostUpdate",
        sample_point = "PostUpdate",
        entity = ?entity,
        component = ?DebugName::type_name::<C>(),
        value = ?value,
        "saved value for timeline switch blend"
    );
    deferred.insert(entity, PreviousVisual(value));
}

/// Removes the state a switch kept for one corrected type once its window ends.
///
/// A saved value that is still there when the window ends belongs to a component
/// the destination timeline never presented, and nothing will use it now; a
/// correction still there is converging, and the apply owns it.
fn remove_switch_state_erased<C, D>(entity: Entity, deferred: &mut DeferredEntityCommands)
where
    C: SyncComponent + Diffable<D>,
    D: Send + Sync + 'static,
{
    deferred.remove::<PreviousVisual<C>>(entity);
}

/// Records the correction for the jump a saved value measures, for one
/// corrected type.
///
/// This is the one place a [`VisualCorrection`] is created. Every jump is
/// measured the same way — the value that was on screen before it, against the
/// value that is on screen now — whether the jump came from a rollback or from a
/// timeline switch, so one handler covers both. The only difference between them
/// is how the resulting error is decayed, which the window decides later.
///
/// Runs in `PostUpdate`, after frame interpolation has written the value this
/// frame renders, so the measurement is against exactly what the entity would
/// show without a correction. That is what makes the blend land on the saved
/// value: the apply adds the error to this value, and the error is the gap
/// between it and the saved one.
///
/// `previous` stays in place when the destination timeline has not presented the
/// component yet, so the correction is recorded once it does. Nothing else
/// removes it: a component can be removed and added back, and the jump that
/// causes is measured from the same saved value.
fn create_visual_correction_erased<C, D>(
    world: UnsafeWorldCell,
    archetype: &Archetype,
    start_secs: f32,
    deferred: &mut DeferredEntityCommands,
) where
    C: SyncComponent + Diffable<D>,
    D: Debug + Send + Sync + 'static,
{
    for entity in archetype.entities() {
        let entity_id = entity.id();
        let Ok(entity_cell) = world.get_entity(entity_id) else {
            continue;
        };
        // SAFETY: the caller declares read access to the saved value, the live
        // value, and the blend marker.
        let Some(previous) =
            (unsafe { entity_cell.get::<PreviousVisual<C>>() }).map(|previous| previous.0.clone())
        else {
            continue;
        };
        let Some(live) = (unsafe { entity_cell.get::<C>() }) else {
            // The destination timeline has not presented this component, so
            // there is nothing to measure against yet. A switch window is what
            // is waiting for it, and the window's length bounds the wait;
            // without one nothing will use the saved value, so it goes rather
            // than lingering until despawn and then being added to whatever the
            // value becomes when it returns.
            let waiting_for_window = entity_cell.contains::<SwitchBlend>();
            if !waiting_for_window {
                deferred.remove::<PreviousVisual<C>>(entity_id);
            }
            continue;
        };
        // `diff(new)` is `new - self`, so this is `previous - live`: the apply
        // adds it to the destination, and the entity keeps rendering the value
        // it had before the jump.
        let error: D = live.diff(&previous);
        trace!(
            target: "lightyear_debug::prediction",
            kind = "visual_correction_created",
            schedule = "PostUpdate",
            sample_point = "PostUpdate",
            entity = ?entity_id,
            component = ?DebugName::type_name::<C>(),
            previous = ?previous,
            current_visual = ?live,
            error = ?error,
            "created visual correction"
        );
        deferred.insert(entity_id, VisualCorrection::<D> { error, start_secs });
        deferred.remove::<PreviousVisual<C>>(entity_id);
    }
}

/// The archetypes that can produce a correction, and which registered
/// corrections apply to each.
///
/// Only archetypes that hold a [`PreviousVisual<C>`] are worth visiting, and a
/// saved value is rare next to the entities a client simulates. Walking every
/// archetype each frame to find them is wasted work, and scanning a component
/// column per archetype is more than is needed: the archetype itself says which
/// kinds are correctable, because component sets are uniform per archetype.
///
/// Cached by archetype generation, so archetypes created by a spawn, an insert,
/// or a switch's marker surgery are picked up on the next frame.
pub(crate) struct CorrectionArchetypeCache {
    generation: ArchetypeGeneration,
    correction_count: usize,
    archetypes: Vec<CorrectionArchetype>,
}

impl Default for CorrectionArchetypeCache {
    fn default() -> Self {
        Self {
            generation: ArchetypeGeneration::initial(),
            correction_count: 0,
            archetypes: Vec::new(),
        }
    }
}

/// One cached archetype: the corrections its saved values can produce.
struct CorrectionArchetype {
    id: ArchetypeId,
    /// Cloned out of the registry so the per-frame pass needs no lookup.
    corrections: Vec<ErasedPostRollbackCorrection>,
}

impl CorrectionArchetypeCache {
    fn update(&mut self, archetypes: &Archetypes, registry: &PredictionRegistry) {
        let correction_count = registry.post_rollback_corrections().count();
        if self.correction_count != correction_count {
            // Corrections registered after the cache was filled: start over so
            // existing archetypes are considered for them too.
            self.generation = ArchetypeGeneration::initial();
            self.archetypes.clear();
            self.correction_count = correction_count;
        }
        let old_generation = core::mem::replace(&mut self.generation, archetypes.generation());
        for archetype in archetypes[old_generation..].iter() {
            let corrections: Vec<_> = registry
                .post_rollback_corrections()
                .filter(|correction| archetype.contains(correction.previous_visual_component_id))
                .collect();
            if corrections.is_empty() {
                continue;
            }
            self.archetypes.push(CorrectionArchetype {
                id: archetype.id(),
                corrections,
            });
        }
    }
}

/// Records a correction for every entity that has a saved value to measure, on
/// every corrected type.
///
/// Runs in `PostUpdate` after frame interpolation and before
/// [`RollbackSystems::VisualCorrection`], which is the apply that adds the
/// errors this writes. See [`create_visual_correction_erased`] for the
/// measurement.
pub(crate) fn create_visual_corrections(
    correction_world: CorrectionWorld,
    registry: Res<PredictionRegistry>,
    time: Res<Time<Virtual>>,
    mut cache: Local<CorrectionArchetypeCache>,
    mut commands: Commands,
) {
    let world = correction_world.world();
    let start_secs = time.elapsed_secs();
    cache.update(world.archetypes(), &registry);
    let mut deferred = DeferredEntityCommands::default();
    for cached in &cache.archetypes {
        let Some(archetype) = world.archetypes().get(cached.id) else {
            continue;
        };
        for correction in &cached.corrections {
            correction.create_visual_correction(world, archetype, start_secs, &mut deferred);
        }
    }
    deferred.apply(&mut commands);
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
/// function, because the error is measured against the value that rule produced
/// for this frame. The error is stored as `D` and given up by the correction
/// function registered through `add_correction`, `add_linear_correction`, or
/// `add_correction_fn`.
pub(crate) fn update_visual_correction<
    C: SyncComponent + Diffable<D>,
    D: Default + Clone + Debug + Send + Sync + 'static,
>(
    time: Res<Time<Virtual>>,
    prediction: Res<PredictionRegistry>,
    manager: Res<PredictionManager>,
    mut query: Query<(
        Entity,
        &mut C,
        &mut VisualCorrection<D>,
        Option<&CorrectionPolicy>,
        Option<&SwitchBlend>,
    )>,
    mut commands: Commands,
) {
    let dt = time.delta();
    let now = time.elapsed_secs();
    let global = &manager.correction_policy;
    query.iter_mut().for_each(
        |(entity, mut component, mut visual_correction, override_policy, blend_marker)| {
            let dt_secs = dt.as_secs_f32();
            let elapsed_secs = visual_correction.elapsed_secs(now);
            let policy_ratio = override_policy
                .map_or(global, |policy| policy)
                .lerp_ratio(elapsed_secs, dt);
            // A live window's ease curve shapes the decay, so the length and
            // curve the caller asked for are what the transition looks like. The
            // curve never gives the error up *faster* than the entity's own
            // correction would: its tail is steep, and a jump arriving late in
            // the window (a rollback during the blend) would otherwise be dumped
            // into a single frame. The policy is the floor, the curve the
            // ceiling, and a converged error ends the blend early.
            let r = blend_marker
                .filter(|marker| !marker.is_expired(now))
                .and_then(|marker| marker.curve_keep(now, dt_secs))
                .map_or(policy_ratio, |curve_ratio| curve_ratio.max(policy_ratio));
            let previous_error = visual_correction.error.clone();
            let mut error_as_component = C::base_value();
            error_as_component.apply_diff(&previous_error);
            // "Small enough to stop applying" is the same test for both
            // schedules: the error has converged, so there is nothing left to
            // carry and the next jump will record a fresh one.
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
                .update_correction::<C, D>(previous_error.clone(), r)
                .expect("No correction function was found. Call add_correction, add_linear_correction, or add_correction_fn for this component.");
            component.apply_diff(&new_error);
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
        },
    );
}

/// A curve that gives a correction error up over time.
///
/// Every error is a recorded jump, and this is how it is given up. The curves
/// come in two shapes:
///
/// * **Unbounded**: [`Exponential`](Self::Exponential) has no end of its own —
///   `decay_ratio` of the error is left after each `decay_period_secs`, so it only
///   ever approaches zero. This is the shape for reconciliation, where the aim is
///   to catch up with the server; it is what [`CorrectionPolicy`] uses.
/// * **Bounded**: the rest are normalised over a window, reaching zero at its
///   end. This is the shape for a timeline switch, where the transition is meant
///   to be seen and to finish.
///
/// A bounded curve gets its length from the window it runs over
/// ([`SwitchBlend`]'s duration) rather than from the variant, so there is one
/// place to set it. An exponential carries its own period, because that period
/// is what defines its shape; a window still bounds it, and the error converges
/// when the window ends.
#[derive(Debug, Clone, Copy, PartialEq, Reflect)]
pub enum CorrectionEase {
    /// Constant speed.
    Linear,
    /// Slow start and slow finish.
    ///
    /// A flat start hides the velocity step when the entity was already moving,
    /// but it holds almost the whole correction for the first frames — see
    /// [`EaseOutCubic`](Self::EaseOutCubic) — which reads as a stop on a blend
    /// that starts from a large gap.
    Smoothstep,
    /// Fast start, slow finish.
    ///
    /// An error is given up from the value on screen towards the destination, so
    /// a curve that starts fast releases a visible amount on the first frame and
    /// settles gradually. A flat-starting curve instead keeps almost the whole
    /// gap for those frames — a frame of a one second `Smoothstep` window
    /// releases under 1% of it — which reads as the entity stopping and then
    /// lurching.
    EaseOutCubic,
    /// Slow start, fast middle, slow finish.
    EaseInOutCubic,
    /// `decay_ratio` of the error remains after each `decay_period_secs` seconds.
    ///
    /// Unlike the curves above this one has no end of its own, so it is the
    /// window it runs in that ends it — see [`Self::is_unbounded`]. Its shape is
    /// its parameters: it gives up the same fraction every frame, rather than
    /// following a curve.
    ///
    /// This is [`Self::default`], and the shape is the one a blend wants: the
    /// gap is released at a steady rate from the first frame instead of being
    /// held while the curve warms up.
    Exponential {
        /// Fraction of the error left after one `decay_period_secs`.
        decay_ratio: f32,
        /// Time for the error to fall to `decay_ratio` of its value.
        decay_period_secs: f32,
    },
}

impl Default for CorrectionEase {
    /// The tuned blend shape: half the remaining error every 200 ms.
    ///
    /// Half the error goes in the first 200 ms, three quarters in 400 ms, and so
    /// on, so a blend releases a little under 6% of its gap per 60 Hz frame from
    /// the very first one. That is enough to keep a moving entity moving through
    /// a switch, where a flat-starting curve would hold nearly all of the gap.
    ///
    /// A struct variant cannot carry `#[default]`, so this is written out rather
    /// than derived — which keeps it the single definition of the default, used
    /// by both [`CorrectionPolicy`] and
    /// [`TimelineSwitchSettings`](crate::switch::TimelineSwitchSettings).
    fn default() -> Self {
        Self::Exponential {
            decay_ratio: 0.5,
            decay_period_secs: 0.2,
        }
    }
}

impl CorrectionEase {
    /// All bounded variants, for UI pickers. [`Exponential`](Self::Exponential)
    /// is parameterised, so it is not part of the rotation.
    pub const ALL: [CorrectionEase; 4] = [
        CorrectionEase::Linear,
        CorrectionEase::Smoothstep,
        CorrectionEase::EaseOutCubic,
        CorrectionEase::EaseInOutCubic,
    ];

    /// Short display name, for UI pickers.
    pub fn name(&self) -> &'static str {
        match self {
            CorrectionEase::Linear => "Linear",
            CorrectionEase::Smoothstep => "Smoothstep",
            CorrectionEase::EaseOutCubic => "EaseOutCubic",
            CorrectionEase::EaseInOutCubic => "EaseInOutCubic",
            CorrectionEase::Exponential { .. } => "Exponential",
        }
    }

    /// True when the curve only approaches zero instead of reaching it.
    pub fn is_unbounded(&self) -> bool {
        matches!(self, CorrectionEase::Exponential { .. })
    }

    /// Fraction of the error still left, `elapsed` seconds in, for a curve
    /// running over a `duration`-long window.
    ///
    /// `duration` bounds every curve; an unbounded one ignores it for its shape
    /// and is cut off by it.
    pub fn remaining(&self, elapsed: f32, duration: f32) -> f32 {
        match self {
            CorrectionEase::Exponential {
                decay_ratio,
                decay_period_secs,
            } => {
                if *decay_period_secs <= 0.0 {
                    return 0.0;
                }
                decay_ratio.powf(elapsed / decay_period_secs)
            }
            bounded => {
                if !duration.is_finite() || duration <= 0.0 {
                    return 0.0;
                }
                bounded.bounded_remaining(elapsed / duration)
            }
        }
    }

    /// Per-frame keep ratio for the frame that starts `elapsed` seconds into a
    /// curve running over a `duration`-long window.
    ///
    /// This is `remaining(next) / remaining(prev)`, so the error telescopes along
    /// the curve exactly, whatever the frame rate. The frame covers
    /// `[elapsed, elapsed + dt]`, and counting the frame being decayed is what
    /// makes a blend's first frame already give up `1 - remaining(dt)` of the
    /// gap. Looking backwards instead clamps the previous edge to `0` on that
    /// frame, which makes the keep ratio exactly `1`: the rendered value is put
    /// back onto the one it already had, so the frame does not move at all.
    pub fn keep(&self, elapsed: f32, dt: f32, duration: f32) -> f32 {
        // The exponential's ratio is constant, so it does not need the edges.
        if let CorrectionEase::Exponential { .. } = self {
            return self.remaining(dt, duration);
        }
        let prev = self.remaining(elapsed, duration);
        let next = self.remaining(elapsed + dt, duration);
        if prev > 0.0 {
            (next / prev).clamp(0.0, 1.0)
        } else {
            0.0
        }
    }

    /// Normalised curves only: an exponential has no window to be normalised
    /// over, and `remaining` handles it before reaching here.
    fn bounded_remaining(&self, progress: f32) -> f32 {
        let p = progress.clamp(0.0, 1.0);
        match self {
            CorrectionEase::Linear => 1.0 - p,
            CorrectionEase::Smoothstep => 1.0 - (p * p * (3.0 - 2.0 * p)),
            CorrectionEase::EaseOutCubic => (1.0 - p).powi(3),
            CorrectionEase::EaseInOutCubic => {
                if p < 0.5 {
                    1.0 - 4.0 * p * p * p
                } else {
                    let q = -2.0 * p + 2.0;
                    q * q * q / 2.0
                }
            }
            CorrectionEase::Exponential { .. } => {
                unreachable!("an exponential is unbounded and handled by the caller")
            }
        }
    }
}

/// Decay schedule for [`VisualCorrection`] errors.
///
/// The default schedule lives on the [`PredictionManager`] resource and applies
/// to every corrected entity. Inserting this component on an entity overrides
/// the global schedule for that entity only — that is how one-off corrections can
/// decay on their own tuning without retuning every other correction.
///
/// This is the unbounded, exponential easing ([`CorrectionEase::Exponential`])
/// written as a pair of parameters, and it is what a rollback correction decays
/// on: the aim there is to catch up with the server, not to look deliberate. A
/// live [`SwitchBlend`] window runs its own curve instead, and the two combine by
/// taking whichever gives the error up more slowly at that instant, so a blend is
/// never shortened by this schedule and a rollback arriving late in a window is
/// never dumped by the curve's tail.
#[derive(Component, Debug, Clone, Copy, Reflect)]
pub struct CorrectionPolicy {
    /// How the error is given up.
    ///
    /// The default is the unbounded exponential, tuned in the two parameters
    /// below. A bounded curve ([`CorrectionEase::Smoothstep`] and friends) is
    /// normalised over [`Self::duration`] instead, and reaches zero at its end.
    ease: CorrectionEase,
    /// How long a bounded curve takes, in seconds.
    ///
    /// Ignored by [`CorrectionEase::Exponential`], which carries its own period
    /// and only approaches zero; but still what bounds it when the policy runs
    /// alongside a [`SwitchBlend`] window, whose own length is used instead. A
    /// bounded curve needs it: [`CorrectionEase`] variants are normalised over a
    /// window, so without a duration they would have nothing to run over.
    duration_secs: f32,
}

impl Default for CorrectionPolicy {
    fn default() -> Self {
        Self {
            // The shared default curve, so a rollback and a blend let the error
            // go at the same rate unless a caller tunes one of them.
            ease: CorrectionEase::default(),
            duration_secs: 0.5,
        }
    }
}

impl CorrectionPolicy {
    /// Custom exponential schedule: after each `decay_period_secs`, `decay_ratio` of
    /// the error remains. Insert on an entity to override the global schedule for
    /// that entity only.
    pub fn new(decay_ratio: f32, decay_period_secs: core::time::Duration) -> Self {
        Self {
            ease: CorrectionEase::Exponential {
                decay_ratio,
                decay_period_secs: decay_period_secs.as_secs_f32(),
            },
            ..Self::default()
        }
    }

    /// Any curve, run over `duration_secs`.
    ///
    /// This is how a bounded curve is selected: an exponential ignores the
    /// duration and is only bounded by it, while a bounded one is normalised over
    /// it and reaches zero there.
    pub fn with_ease(ease: CorrectionEase, duration_secs: f32) -> Self {
        Self {
            ease,
            duration_secs,
        }
    }

    /// The curve this policy decays on.
    pub fn ease(&self) -> CorrectionEase {
        self.ease
    }

    /// Returns the lerp constant to use for decaying the error in a framestep-insensitive way.
    ///
    /// For an exponential this is constant, which is the classic framestep-insensitive
    /// exponential decay; see <https://www.youtube.com/watch?v=LSNQuFEDOyQ>.
    /// A bounded curve has no such constant — its ratio depends on where in the
    /// window the frame falls — so it is read from `elapsed`, which the caller
    /// takes from the correction's own recorded start.
    #[inline]
    pub fn lerp_ratio(&self, elapsed_secs: f32, delta: core::time::Duration) -> f32 {
        self.ease
            .keep(elapsed_secs, delta.as_secs_f32(), self.duration_secs)
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
        plugin::InterpolationMarkerPlugin,
        registry::{AppInterpolationExt, InterpolationRegistry},
        rules::InterpolationFns,
    };
    use lightyear_replication::diffable::Diffable as LightyearDiffable;
    use lightyear_replication::prelude::*;

    use super::*;
    use crate::plugin::PredictionMarkerPlugin;
    use crate::registry::{PredictionAppRegistrationExt, PredictionBuilderExt, PredictionRegistry};
    use bevy_time::Fixed;

    fn app_with_replication_markers() -> App {
        let mut app = App::new();
        app.add_plugins((
            StatesPlugin,
            RepliconSharedPlugin {
                auth_method: AuthMethod::None,
            },
            PredictionMarkerPlugin,
            InterpolationMarkerPlugin,
        ));
        // The shared correction system stamps each error with the visual clock,
        // so a bounded curve can advance along its window.
        app.insert_resource(Time::<Virtual>::default());
        app
    }

    #[derive(Component, Clone, Debug, Default, PartialEq)]
    struct CorrectionA(f32);

    #[derive(Component, Clone, Debug, PartialEq)]
    #[component(storage = "SparseSet")]
    struct SparseCorrection(f32);

    #[derive(Component)]
    struct UnrelatedCorrectionComponent;

    impl Ease for CorrectionA {
        fn interpolating_curve_unbounded(start: Self, end: Self) -> impl Curve<Self> {
            FunctionCurve::new(Interval::UNIT, move |t| {
                CorrectionA(start.0 + (end.0 - start.0) * t)
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

    #[test]
    fn correction_registration_adds_frame_interpolation_setup() {
        let mut app = app_with_replication_markers();
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
    }

    /// A converged offset is dropped whether or not a switch window is live.
    /// Nothing re-derives an offset from the value a switch started from, so a
    /// window does not need to keep the correction around: the next jump (a
    /// rollback, or another switch) records a fresh one.
    /// The frame a window opens moves the render. Counting the frame being
    /// decayed is what makes that true: without it the first frame's keep ratio
    /// is exactly `1`, which puts the rendered value back onto the one it already
    /// had and shows the same pose twice.
    #[test]
    fn a_blend_first_frame_gives_up_part_of_the_gap() {
        for ease in [
            CorrectionEase::Linear,
            CorrectionEase::Smoothstep,
            CorrectionEase::EaseOutCubic,
            CorrectionEase::EaseInOutCubic,
        ] {
            let keep = ease.keep(0.0, 1.0 / 60.0, 1.0);
            assert!(
                keep < 1.0,
                "{ease:?} kept everything on the first frame: {keep}"
            );
            // A frame is the smallest step there is, so a curve should not give
            // up the whole window in one frame either.
            assert!(keep > 0.0, "{ease:?} gave up the whole gap at once: {keep}");
        }
        // Linear is the easiest to check by hand: one frame of a one second
        // window gives up exactly that frame's fraction.
        let keep = CorrectionEase::Linear.keep(0.0, 0.05, 1.0);
        assert!((keep - 0.95).abs() < 1e-6, "got {keep}");
    }

    #[test]
    fn converged_correction_is_dropped_even_while_a_blend_is_live() {
        let mut app = app_with_replication_markers();
        app.init_resource::<PredictionRegistry>();
        app.insert_resource(Time::<Virtual>::default());
        app.component::<CorrectionA>().predict().add_correction();
        app.insert_resource(PredictionManager::default());
        let blending = app
            .world_mut()
            .spawn((
                CorrectionA(10.0),
                VisualCorrection::new(CorrectionA(0.0), 0.0),
                SwitchBlend::new(0.0, 0.5, CorrectionEase::Linear),
            ))
            .id();
        let plain = app
            .world_mut()
            .spawn((
                CorrectionA(10.0),
                VisualCorrection::new(CorrectionA(0.0), 0.0),
            ))
            .id();

        app.world_mut()
            .run_system_once(update_visual_correction::<CorrectionA, CorrectionA>)
            .unwrap();

        assert!(
            app.world()
                .get::<VisualCorrection<CorrectionA>>(blending)
                .is_none(),
            "a converged offset goes, window or not"
        );
        assert!(
            app.world()
                .get::<VisualCorrection<CorrectionA>>(plain)
                .is_none(),
            "zero error without a blend is removed too"
        );
        // The window itself is untouched: it still gates re-switching.
        assert!(
            app.world().get::<SwitchBlend>(blending).is_some(),
            "the window lives out its length"
        );
    }
    #[test]
    fn entity_correction_policy_overrides_global() {
        let mut app = app_with_replication_markers();
        app.init_resource::<PredictionRegistry>();
        app.insert_resource(Time::<Virtual>::default());
        app.component::<CorrectionA>().predict().add_correction();
        app.insert_resource(PredictionManager::default());
        // Same unit error, but this entity decays on a 50 ms half schedule
        // instead of the global 200 ms one.
        let overridden = app
            .world_mut()
            .spawn((
                CorrectionA(10.0),
                VisualCorrection::new(CorrectionA(1.0), 0.0),
                CorrectionPolicy::new(0.5, Duration::from_millis(50)),
            ))
            .id();
        let plain = app
            .world_mut()
            .spawn((
                CorrectionA(10.0),
                VisualCorrection::new(CorrectionA(1.0), 0.0),
            ))
            .id();
        app.world_mut()
            .resource_mut::<Time<Virtual>>()
            .advance_by(Duration::from_millis(125));

        app.world_mut()
            .run_system_once(update_visual_correction::<CorrectionA, CorrectionA>)
            .unwrap();

        let fast = app
            .world()
            .get::<VisualCorrection<CorrectionA>>(overridden)
            .unwrap()
            .error
            .0;
        let slow = app
            .world()
            .get::<VisualCorrection<CorrectionA>>(plain)
            .unwrap()
            .error
            .0;
        assert!(fast < 0.3, "override should decay faster, got {fast}");
        assert!(slow > 0.55, "global schedule unchanged, got {slow}");
    }

    /// A bounded curve can be the correction policy too, not just a switch
    /// window's curve. It is normalised over the policy's own duration and
    /// reaches zero there, driven by the clock the error was recorded at.
    /// The cache is filled once and reused, so it has to notice archetypes that
    /// appear later: a saved value moves an entity into a new archetype, and if
    /// the cache never looked again the correction would never be recorded.
    #[test]
    fn correction_cache_picks_up_archetypes_created_later() {
        use bevy_ecs::system::RunSystemOnce;

        let mut app = app_with_replication_markers();
        app.init_resource::<PredictionRegistry>();
        app.component::<CorrectionA>().predict().add_correction();

        // First pass: nothing has a saved value yet, so the cache is empty.
        app.world_mut()
            .run_system_once(create_visual_corrections)
            .unwrap();
        app.world_mut().flush();

        // A saved value appears, which puts the entity in a new archetype.
        let entity = app
            .world_mut()
            .spawn((CorrectionA(10.0), PreviousVisual(CorrectionA(4.0))))
            .id();
        app.world_mut().flush();

        app.world_mut()
            .run_system_once(create_visual_corrections)
            .unwrap();
        app.world_mut().flush();

        assert_eq!(
            app.world()
                .get::<VisualCorrection<CorrectionA>>(entity)
                .map(|correction| correction.error.0),
            Some(-6.0),
            "the newly created archetype must be picked up"
        );
        assert!(
            app.world()
                .get::<PreviousVisual<CorrectionA>>(entity)
                .is_none()
        );
    }

    /// A correction registered after the cache was filled has to be considered
    /// for archetypes that already exist, so the cache is dropped when the
    /// A bounded curve can be the correction policy too, not just a switch
    /// window's curve. It is normalised over the policy's own duration, driven
    /// by the clock the error was recorded at, and the error tracks the curve
    /// exactly however the frames fall.
    #[test]
    fn correction_policy_can_run_a_bounded_curve() {
        let mut app = app_with_replication_markers();
        app.init_resource::<PredictionRegistry>();
        app.component::<CorrectionA>().predict().add_correction();
        app.insert_resource(PredictionManager::default());
        // Half a second linear ramp.
        let duration = 0.5;
        let dt = 16.0 / 1000.0;
        // The app records a correction and applies it in the same frame, so its
        // start is stamped at the clock reading the first apply sees. Do the same
        // here, or the first frame would decay a window that has not started.
        app.world_mut()
            .resource_mut::<Time<Virtual>>()
            .advance_by(Duration::from_secs_f32(dt));
        let start_secs = app.world().resource::<Time<Virtual>>().elapsed_secs();
        let policy = CorrectionPolicy::with_ease(CorrectionEase::Linear, duration);
        let entity = app
            .world_mut()
            .spawn((
                CorrectionA(10.0),
                VisualCorrection::new(CorrectionA(1.0), start_secs),
                policy,
            ))
            .id();
        app.world_mut().flush();

        // Step through the ramp: the error telescopes along the curve rather than
        // accumulating a per-frame factor, so it is always `remaining(elapsed +
        // dt)` — the frame being decayed counts, which is what stops a blend's
        // first frame from holding the render still.
        // The frame that records the correction applies it too, so decay one
        // frame before advancing the clock.
        let mut elapsed = 0.0f32;
        app.world_mut()
            .run_system_once(update_visual_correction::<CorrectionA, CorrectionA>)
            .unwrap();
        assert_eq!(
            app.world()
                .get::<VisualCorrection<CorrectionA>>(entity)
                .map(|correction| correction.error.0),
            Some(1.0 - dt / duration),
            "the frame that records the correction already gives up one frame of it"
        );
        for _ in 0..20 {
            app.world_mut()
                .resource_mut::<Time<Virtual>>()
                .advance_by(Duration::from_secs_f32(dt));
            app.world_mut()
                .run_system_once(update_visual_correction::<CorrectionA, CorrectionA>)
                .unwrap();
            let error = app
                .world()
                .get::<VisualCorrection<CorrectionA>>(entity)
                .map(|correction| correction.error.0);
            elapsed += dt;
            assert!(
                (error.unwrap() - (1.0 - (elapsed + dt) / duration)).abs() < 1e-6,
                "after {elapsed}s of a {duration}s linear ramp: {error:?}"
            );
        }

        // At the end of the ramp the error has converged exactly ...
        app.world_mut()
            .resource_mut::<Time<Virtual>>()
            .advance_by(Duration::from_millis(400));
        app.world_mut()
            .run_system_once(update_visual_correction::<CorrectionA, CorrectionA>)
            .unwrap();
        assert_eq!(
            app.world()
                .get::<VisualCorrection<CorrectionA>>(entity)
                .map(|correction| correction.error.0),
            Some(0.0),
            "a bounded curve reaches zero at the end of its window"
        );
        // ... and the following pass drops it, like any converged correction.
        app.world_mut()
            .resource_mut::<Time<Virtual>>()
            .advance_by(Duration::from_millis(16));
        app.world_mut()
            .run_system_once(update_visual_correction::<CorrectionA, CorrectionA>)
            .unwrap();
        assert!(
            app.world()
                .get::<VisualCorrection<CorrectionA>>(entity)
                .is_none(),
            "a converged correction is dropped"
        );
    }

    #[test]
    fn visual_correction_marks_component_changed() {
        let mut app = app_with_replication_markers();
        app.init_resource::<PredictionRegistry>();
        app.insert_resource(Time::<Virtual>::default());
        app.component::<CorrectionA>().predict().add_correction();
        app.insert_resource(PredictionManager::default());
        let entity = app
            .world_mut()
            .spawn((
                CorrectionA(10.0),
                VisualCorrection::new(CorrectionA(1.0), 0.0),
            ))
            .id();
        app.world_mut().clear_trackers();
        let changed = app
            .world()
            .entity(entity)
            .get_change_ticks::<CorrectionA>()
            .unwrap()
            .changed;

        app.world_mut()
            .run_system_once(update_visual_correction::<CorrectionA, CorrectionA>)
            .unwrap();

        assert_ne!(
            app.world()
                .entity(entity)
                .get_change_ticks::<CorrectionA>()
                .unwrap()
                .changed,
            changed
        );
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

        let mut app = app_with_replication_markers();
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

        app.world_mut().clear_trackers();
        let live_frame_history_tick = app
            .world()
            .entity(live)
            .get_change_ticks::<FrameInterpolationHistory<CorrectionA>>()
            .unwrap()
            .changed;
        let removed_frame_history_tick = app
            .world()
            .entity(removed)
            .get_change_ticks::<FrameInterpolationHistory<CorrectionA>>()
            .unwrap()
            .changed;

        app.world_mut()
            .run_system_once(update_frame_interpolation_post_rollback)
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
        assert_ne!(
            app.world()
                .entity(live)
                .get_change_ticks::<FrameInterpolationHistory<CorrectionA>>()
                .unwrap()
                .changed,
            live_frame_history_tick,
            "repair must retain Bevy change detection for surviving components"
        );

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
        assert_ne!(
            app.world()
                .entity(removed)
                .get_change_ticks::<FrameInterpolationHistory<CorrectionA>>()
                .unwrap()
                .changed,
            removed_frame_history_tick,
            "repair must retain Bevy change detection for removed live components"
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
        app.init_resource::<PredictionRegistry>();
        app.init_resource::<ComponentRegistry>();
        app.local_rollback::<SparseCorrection>();
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
            .run_system_once(update_frame_interpolation_post_rollback)
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

    // A capture without its live value (archetype mid-assembly: the value has
    // not replicated yet) carries nothing correctable and must be skipped,
    // not crash rule resolution.
    #[test]
    fn post_rollback_correction_skips_capture_without_live_value() {
        let mut app = app_with_replication_markers();
        app.init_resource::<PredictionRegistry>();
        app.init_resource::<InterpolationRegistry>();
        app.insert_resource(Time::<Fixed>::from_duration(Duration::from_secs(1)));
        app.insert_resource(LocalTimeline::default());
        app.world_mut()
            .resource_mut::<LocalTimeline>()
            .apply_delta(10);

        app.component::<CorrectionA>().predict().add_correction();

        app.world_mut().spawn((PreviousVisual(CorrectionA(12.0)),));

        app.world_mut()
            .run_system_once(create_visual_corrections)
            .unwrap();
        let mut query = app.world_mut().query::<&VisualCorrection<CorrectionA>>();
        assert!(query.iter(app.world()).next().is_none());
    }

    /// A saved value with no live component to measure against is only kept
    /// while a switch window is waiting for that component. Without one nothing
    /// will ever use it, so it is discarded instead of lingering until despawn
    /// and then being added to whatever the value becomes when it returns.
    #[test]
    fn creation_discards_a_saved_value_nobody_is_waiting_for() {
        let mut app = app_with_replication_markers();
        app.init_resource::<PredictionRegistry>();
        app.component::<CorrectionA>().predict().add_correction();

        // No live component: the destination timeline has not presented it.
        let unwatched = app.world_mut().spawn(PreviousVisual(CorrectionA(8.0))).id();
        // The same, but a switch window is waiting for the component.
        let watched = app
            .world_mut()
            .spawn((
                PreviousVisual(CorrectionA(8.0)),
                SwitchBlend::new(0.0, 0.5, CorrectionEase::Linear),
            ))
            .id();
        app.world_mut().flush();

        app.world_mut()
            .run_system_once(create_visual_corrections)
            .unwrap();
        app.world_mut().flush();

        let world = app.world();
        assert!(
            world
                .get::<PreviousVisual<CorrectionA>>(unwatched)
                .is_none(),
            "nothing was waiting for this value"
        );
        assert!(
            world.get::<PreviousVisual<CorrectionA>>(watched).is_some(),
            "the window must keep waiting for the component it will blend"
        );
    }
}
