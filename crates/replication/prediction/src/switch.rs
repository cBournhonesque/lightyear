//! Client-side switching between the prediction and interpolation timelines.
//!
//! Gameplay code (proximity, pickup ownership, ...) inserts a [`TimelineSwitch`]
//! component to move one entity between the prediction timeline (client-ahead
//! simulation) and the interpolation timeline (delayed server presentation).
//!
//! This can be useful to temporarily predict remote players around you so that
//! you can physically collide/interact with them, while keeping the rest of
//! the world interpolated.
//!
//! To run, insert a [`TimelineSwitch`] on the entity.
//!```rust,ignore
//! // To a named timeline:
//! commands.entity(entity).insert(TimelineSwitch::to_predicted());
//! // ...or blend over an explicit window instead of the default:
//! commands.entity(entity).insert(
//!     TimelineSwitch::to_predicted().with_transition_secs(1.0),
//! );
//! // Or flip to whichever timeline the entity is not on:
//! commands.entity(entity).insert(TimelineSwitch::default());
//! ```
//!
//! [`TimelineSwitch`]: crate::switch::TimelineSwitch

// # Order of operations
//
// A request is handled over two frames. Say the request is inserted on frame F
// (gameplay code usually inserts it from `Update`):
//
// 1. `save_previous_visuals` runs in `PostUpdate` of frame F, after frame
//    interpolation and after the correction apply, so what it reads is what the
//    frame shows. It saves that value as
//    [`PreviousVisual`](crate::correction::PreviousVisual) — for a switch to the
//    interpolation timeline, the only thing that can record it — and resolves
//    the request into a `PendingSwitch`: the timeline it names (or the one a
//    flip picks) and the blend schedule to run, committed for the next frame.
//    The request itself is dropped, so what is carried to the next frame is the
//    resolution and nothing else. The entity's timeline does not change yet. A
//    switch that snaps saves nothing.
// 2. `apply_saved_switches` runs in `PreUpdate` of frame F+1, after
//    replication receive and before the rollback check. It swaps the
//    [`Predicted`] / [`Interpolated`] markers and removes the request. For a
//    switch to the prediction timeline it also asks for a forced rollback, so
//    that in this same frame the world rewinds to K and replays under the new
//    marker, leaving fresh simulated values in place.
// 3. The shared creation system in [`crate::correction`] records the blend's
//    error in `PostUpdate` of frame F+1, after frame interpolation: the jump
//    from the value saved in step 1 to whatever the destination timeline has
//    now. It runs every frame, and keeps the window's error up to date for as
//    long as the window is live.
//
// `expire_switch_blends` ends the window and clears what it kept.
//
// # How a blend relates to `VisualCorrection`
//
// A blend *is* a correction. There is one error per corrected component, one
// record of one jump:
//
// ```text
// rendered = destination + error    (the apply adds the error every frame)
// error    = previous - destination (recorded once, when the window opens)
// ```
//
// and one decay that gives it up. The switch does not own a second error and
// does not stack its error on top of a correction: it records the same kind of
// measurement the rollback machinery records, in the same
// [`PreviousVisual`](crate::correction::PreviousVisual) slot, and its window
// decides how that error is decayed. When a new correction is applied while
// a previous one was running, the highest decay ratio between the two is used
// (the one that would give the biggest error).

use crate::correction::{CorrectionEase, CorrectionWorld};
use crate::manager::StateRollbackMetadata;
use crate::plugin::PredictionSystems;
use crate::registry::PredictionRegistry;
use crate::rollback::RollbackSystems;
use bevy_app::prelude::*;
use bevy_ecs::prelude::*;
use bevy_reflect::Reflect;
use bevy_time::{Time, Virtual};
use lightyear_core::interpolation::Interpolated;
use lightyear_core::prediction::Predicted;
use lightyear_frame_interpolation::FrameInterpolate;
use lightyear_replication::deferred_entity::DeferredEntityCommands;
use lightyear_replication::prelude::ReplicationSystems;
use tracing::{trace, warn};

/// Direction of a [`TimelineSwitch`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwitchDirection {
    /// Move the entity to the prediction timeline.
    ToPredicted,
    /// Move the entity to the interpolation timeline.
    ToInterpolated,
}

/// Default blend length for timeline switches, in seconds.
///
/// The drain resolves a request without an explicit override against this
/// resource, so games tune the feel in one place instead of threading a
/// duration through every gameplay insert site. Insert (or mutate) it to change
/// the default; per-request [`TimelineSwitch::with_transition_secs`] wins.
/// Same for the ease curve ([`TimelineSwitch::with_ease`]).
#[derive(Resource, Debug, Clone, Copy, PartialEq)]
pub struct TimelineSwitchSettings {
    /// Blend length used when a request carries no explicit override.
    pub default_transition_secs: f32,
    /// Ease curve used when a request carries no explicit override.
    pub default_ease: CorrectionEase,
}

impl Default for TimelineSwitchSettings {
    fn default() -> Self {
        Self {
            // Long enough for a blend to converge: with the default curve, half
            // the gap goes every 200 ms, so a second leaves about 3% of it, which
            // the correction policy finishes.
            default_transition_secs: 1.0,
            default_ease: CorrectionEase::default(),
        }
    }
}

/// Request to move an entity between the prediction and interpolation timelines.
///
/// Insert it on the entity — the switch pipeline is what adds or removes
/// [`Predicted`] / [`Interpolated`], so callers never touch those directly:
/// ```rust,ignore
/// // To a named timeline:
/// commands.entity(entity).insert(TimelineSwitch::to_predicted());
/// // ...or blend over an explicit window instead of the default:
/// commands.entity(entity).insert(
///     TimelineSwitch::to_predicted().with_transition_secs(1.0),
/// );
/// // Or flip to whichever timeline the entity is not on:
/// commands.entity(entity).insert(TimelineSwitch::default());
/// ```
///
/// You can customize the blend length and ease curve per request, which overrides the
/// [`TimelineSwitchSettings`] default: `<= 0` (or non-finite) snaps instantly,
/// otherwise the jump is smoothed over the window and further requests for the
/// entity are ignored until the switch finishes.
#[derive(Component, Debug, Clone, Copy, PartialEq, Default)]
pub struct TimelineSwitch {
    /// The timeline to move to, or `None` to flip to whichever timeline the
    /// entity is not on. A bare [`TimelineSwitch::default()`] flips; an entity
    /// on neither timeline has nothing to flip from and the request is dropped.
    direction: Option<SwitchDirection>,
    /// Blend length in seconds, overriding
    /// [`TimelineSwitchSettings::default_transition_secs`].
    transition_secs: Option<f32>,
    /// Ease curve, overriding [`TimelineSwitchSettings::default_ease`].
    ease: Option<CorrectionEase>,
}

impl TimelineSwitch {
    /// Switch this entity from interpolated to predicted
    pub fn to_predicted() -> Self {
        Self {
            direction: Some(SwitchDirection::ToPredicted),
            ..Default::default()
        }
    }

    /// Switch this entity from predicted to interpolated
    pub fn to_interpolated() -> Self {
        Self {
            direction: Some(SwitchDirection::ToInterpolated),
            ..Default::default()
        }
    }

    /// Override the [`TimelineSwitchSettings`] default blend length for this
    /// request. `<= 0` (or non-finite) snaps instantly instead of blending.
    pub fn with_transition_secs(mut self, transition_secs: f32) -> Self {
        self.transition_secs = Some(transition_secs);
        self
    }

    /// Override the [`TimelineSwitchSettings`] default ease curve for this
    /// request.
    pub fn with_ease(mut self, ease: CorrectionEase) -> Self {
        self.ease = Some(ease);
        self
    }

    /// The timeline this request names, or `None` to flip to whichever timeline
    /// the entity is not on.
    pub fn direction(&self) -> Option<SwitchDirection> {
        self.direction
    }

    /// Per-request blend-length override, if any. `None` takes
    /// [`TimelineSwitchSettings::default_transition_secs`] when the request is
    /// saved.
    pub fn transition_override(&self) -> Option<f32> {
        self.transition_secs
    }

    /// Per-request ease-curve override, if any. `None` takes
    /// [`TimelineSwitchSettings::default_ease`] when the request is saved.
    pub fn ease_override(&self) -> Option<CorrectionEase> {
        self.ease
    }

    /// Resolves the request against the entity's markers and `settings` into the
    /// [`PendingSwitch`] the apply pass serves.
    fn resolve(
        &self,
        is_predicted: bool,
        is_interpolated: bool,
        settings: &TimelineSwitchSettings,
    ) -> Option<PendingSwitch> {
        let direction = match self.direction {
            Some(direction) => direction,
            None if is_predicted => SwitchDirection::ToInterpolated,
            None if is_interpolated => SwitchDirection::ToPredicted,
            None => return None,
        };
        Some(PendingSwitch {
            direction,
            total_secs: self
                .transition_secs
                .unwrap_or(settings.default_transition_secs),
            ease: self.ease.unwrap_or(settings.default_ease),
        })
    }
}

/// A resolved [`TimelineSwitch`], committed for the next frame's apply pass.
///
/// While this is present, a new [`TimelineSwitch`] is ignored, exactly as it is
/// while a [`SwitchBlend`] window is running.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub(crate) struct PendingSwitch {
    /// The timeline to move to, resolved against the entity's markers.
    direction: SwitchDirection,
    /// Blend length in seconds, resolved against [`TimelineSwitchSettings`].
    /// `<= 0` (or non-finite) snaps.
    total_secs: f32,
    /// Ease curve shaping the blend, resolved.
    ease: CorrectionEase,
}

impl PendingSwitch {
    /// Whether this switch blends instead of snapping.
    fn blends(&self) -> bool {
        self.total_secs.is_finite() && self.total_secs > 0.0
    }
}

/// Marks an entity while a switch is ongoing.
///
///  While present, a new [`TimelineSwitch`] inserted on the entity is dropped.
#[derive(Component, Debug, Clone, Copy, PartialEq, Reflect)]
pub struct SwitchBlend {
    /// Visual-clock reading the blend started at, in seconds.
    start_secs: f32,
    /// Requested blend length in seconds.
    total_secs: f32,
    /// Ease curve shaping the convergence over the window.
    ease: CorrectionEase,
}

impl SwitchBlend {
    /// Stamps the shared window for one switch: `now_secs` is the current
    /// visual-clock reading, `total_secs` the requested blend length.
    pub(crate) fn new(now_secs: f32, total_secs: f32, ease: CorrectionEase) -> Self {
        Self {
            start_secs: now_secs,
            total_secs,
            ease,
        }
    }

    /// Requested blend length in seconds.
    pub fn total_secs(&self) -> f32 {
        self.total_secs
    }

    /// Blend time elapsed in seconds at the `now_secs` clock reading.
    pub fn elapsed_secs(&self, now_secs: f32) -> f32 {
        (now_secs - self.start_secs).max(0.0)
    }

    /// True once the window has run out at the `now_secs` clock reading.
    pub fn is_expired(&self, now_secs: f32) -> bool {
        self.elapsed_secs(now_secs) >= self.total_secs
    }

    /// Fraction of the window's recorded jump to keep over the frame of
    /// `dt_secs` that starts at `now_secs`, or `None` when the window cannot
    /// shape a decay.
    ///
    /// The error decays rather than being recomputed, so this is the per-frame
    /// scale factor: `remaining(next) / remaining(prev)` follows the ease curve
    /// exactly, whatever the frame rate. The frame covers
    /// `[now, now + dt]`, counting the frame being decayed: a window's first
    /// frame already gives up `1 - remaining(dt)` of the gap instead of keeping
    /// all of it, so the render keeps moving through the switch.
    ///
    /// `None` means the caller should fall back to the entity's own
    /// [`CorrectionPolicy`](crate::correction::CorrectionPolicy): the window has
    /// run out, or its length is unusable.
    pub(crate) fn curve_keep(&self, now_secs: f32, dt_secs: f32) -> Option<f32> {
        let total = self.total_secs;
        if !total.is_finite() || total <= 0.0 {
            return None;
        }
        let elapsed = self.elapsed_secs(now_secs);
        if elapsed >= total {
            return None;
        }
        Some(self.ease.keep(elapsed, dt_secs, total))
    }

    /// Ease curve shaping the convergence over the window.
    pub fn ease(&self) -> CorrectionEase {
        self.ease
    }
}

/// Saves the value on screen for every switch requested this frame.
///
/// Runs in `PostUpdate` after frame interpolation and after the correction
/// apply, so what it reads is what this frame renders, whatever point of the
/// frame the request was inserted from.
///
/// Requests for entities that are already blending, or that already have a
/// committed switch waiting for its apply pass, are dropped.
pub(crate) fn save_previous_visuals(
    correction_world: CorrectionWorld,
    registry: Res<PredictionRegistry>,
    settings: Res<TimelineSwitchSettings>,
    mut requests: Query<(
        Entity,
        &TimelineSwitch,
        Has<Predicted>,
        Has<Interpolated>,
        Has<SwitchBlend>,
        Has<PendingSwitch>,
    )>,
    mut commands: Commands,
) {
    let world = correction_world.world();
    for (entity, switch, is_predicted, is_interpolated, is_blending, is_committed) in &mut requests
    {
        // Saving while a blend is running would fight the window that is still
        // converging, so the request waits for it to end. A policy that
        // re-inserts every frame produces this routinely, so it is not a user
        // error; it is also what makes a repeated request harmless.
        if is_blending {
            trace!(?entity, "dropping switch while a blend is active");
            commands.entity(entity).remove::<TimelineSwitch>();
            continue;
        }
        // A committed switch owns the entity until it is applied. Resolving a
        // request on top of it would clobber the resolution the save pass already
        // recorded a value for, leaving that value to be measured as a jump that
        // never happened.
        if is_committed {
            trace!(
                ?entity,
                "dropping switch while one is waiting to be applied"
            );
            commands.entity(entity).remove::<TimelineSwitch>();
            continue;
        }
        // A flip resolves against the markers the entity has now, so it is the
        // same value the checks below compare against.
        let Some(pending) = switch.resolve(is_predicted, is_interpolated, &settings) else {
            warn!(
                ?entity,
                ?is_predicted,
                ?is_interpolated,
                "dropping invalid switch to the other timeline: entity is on neither timeline"
            );
            commands.entity(entity).remove::<TimelineSwitch>();
            continue;
        };
        match pending.direction {
            SwitchDirection::ToPredicted if !is_interpolated => {
                warn!(
                    ?entity,
                    ?is_predicted,
                    ?is_interpolated,
                    "dropping invalid switch to predicted: entity must be interpolated"
                );
                commands.entity(entity).remove::<TimelineSwitch>();
                continue;
            }
            SwitchDirection::ToInterpolated if !is_predicted => {
                warn!(
                    ?entity,
                    ?is_predicted,
                    ?is_interpolated,
                    "dropping invalid switch to interpolated: entity must be predicted"
                );
                commands.entity(entity).remove::<TimelineSwitch>();
                continue;
            }
            _ => {}
        }
        let mut saves = DeferredEntityCommands::default();
        // Nothing is saved for a switch that snaps: with no window there is
        // nothing to measure a correction from, and a saved value would be left
        // behind with no reader. A switch to the prediction timeline still gets
        // the rollback's own correction, which is what a snap wants.
        if pending.blends() {
            for correction in registry.post_rollback_corrections() {
                (correction.save_previous_visual_for_switch_fn())(
                    world,
                    entity,
                    pending.direction,
                    &mut saves,
                );
            }
        }
        let total_secs = pending.total_secs;
        let direction = pending.direction;
        saves.remove::<TimelineSwitch>(entity);
        saves.insert(entity, pending);
        saves.apply(&mut commands);
        trace!(?entity, ?direction, total_secs, "saved switch visual");
    }
}

/// Swaps the timeline markers of every switch the save pass committed last frame.
pub(crate) fn apply_saved_switches(
    time: Res<Time<Virtual>>,
    mut pending: Query<(Entity, &PendingSwitch)>,
    metadata: Option<ResMut<StateRollbackMetadata>>,
    mut commands: Commands,
) {
    if pending.is_empty() {
        return;
    }
    let now_secs = time.elapsed_secs();
    let mut forced_rollback = false;
    for (entity, switch) in &mut pending {
        let mut deferred = DeferredEntityCommands::default();
        match switch.direction {
            SwitchDirection::ToPredicted => {
                deferred.remove::<Interpolated>(entity);
                deferred.insert(entity, Predicted);
                // Predicted entities render through frame interpolation, and a
                // blend only shows while `FrameInterpolate` is present. (The
                // saved value requires it too; on a snapping switch there is no
                // saved value, hence the explicit insert.)
                deferred.insert(entity, FrameInterpolate);
                // Forced rollback at K: the rollback check below consumes this
                // before its policy scan, rewinds the world to confirmed data at
                // K, and replays forward under the new marker, so the window
                // blends toward fresh simulation. Before the first sync there is
                // no K to rewind to, so the switch proceeds without a force and
                // the next natural rollback corrects.
                forced_rollback = true;
            }
            SwitchDirection::ToInterpolated => {
                deferred.remove::<Predicted>(entity);
                deferred.insert(entity, Interpolated);
                // Delayed interpolation writes the visual value directly every
                // render frame, and frame interpolation would restore its stale
                // history over that write and freeze the render, so the switch
                // owns this removal.
                deferred.remove::<FrameInterpolate>(entity);
            }
        }
        // The switch has been served: it is one-shot, so the committed state goes
        // with the markers it swapped.
        deferred.remove::<PendingSwitch>(entity);
        if switch.blends() {
            deferred.insert(
                entity,
                SwitchBlend::new(now_secs, switch.total_secs, switch.ease),
            );
        }
        deferred.apply(&mut commands);
        trace!(
            ?entity,
            direction = ?switch.direction,
            blends = switch.blends(),
            "applied timeline switch"
        );
    }
    if forced_rollback
        && let Some(mut metadata) = metadata
        && let Some(k) = metadata.last_processed_confirmed_tick()
    {
        metadata.request_forced_rollback(k);
        trace!(
            rollback_tick = k.0,
            "timeline switch requested forced rollback"
        );
    }
}

/// Lifts expired blend markers and clears the window's state.
pub(crate) fn expire_switch_blends(
    registry: Res<PredictionRegistry>,
    time: Res<Time<Virtual>>,
    blends: Query<(Entity, &SwitchBlend)>,
    mut commands: Commands,
) {
    let now = time.elapsed_secs();
    let mut deferred = DeferredEntityCommands::default();
    for (entity, blend) in &blends {
        if !blend.is_expired(now) {
            continue;
        }
        commands.entity(entity).remove::<SwitchBlend>();
        // The window is over, so the values it blended from have no reader
        // left. A component the destination never presented would otherwise
        // keep them on the entity for good.
        for correction in registry.post_rollback_corrections() {
            (correction.remove_switch_state_fn())(entity, &mut deferred);
        }
    }
    deferred.apply(&mut commands);
}

/// Registers the timeline-switch pipeline on the prediction client.
///
/// All systems only run for client/P2P topologies (same gate as the rest of
/// prediction): host-servers stay authoritative and never switch.
pub(crate) fn add_timeline_switch_systems(app: &mut App) {
    app.init_resource::<TimelineSwitchSettings>();
    app.add_systems(
        PreUpdate,
        apply_saved_switches
            .after(ReplicationSystems::Receive)
            .in_set(PredictionSystems::Rollback)
            .before(RollbackSystems::Check)
            .run_if(crate::plugin::should_run),
    );
    app.add_systems(
        PostUpdate,
        (
            // The correction that starts a blend is recorded by the shared
            // system in `crate::correction`, which runs between frame
            // interpolation and the correction apply.
            //
            // A window is lifted before new requests are looked at, so a request
            // that arrives on the frame its window expires is served then rather
            // than a frame later. Without this the two systems have no relative
            // order, and which one runs first would decide that.
            expire_switch_blends
                .in_set(PredictionSystems::All)
                .after(RollbackSystems::VisualCorrection)
                .before(save_previous_visuals),
            // Saving reads what the frame renders, so it runs after the apply
            // has added this frame's correction.
            save_previous_visuals
                .in_set(PredictionSystems::All)
                .after(RollbackSystems::VisualCorrection),
        )
            .run_if(crate::plugin::should_run),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::correction::{PreviousVisual, VisualCorrection};
    use crate::plugin::PredictionMarkerPlugin;
    use crate::predicted_history::PredictionHistory;
    use crate::registry::{PredictionBuilderExt, PredictionRegistry};
    use bevy_ecs::system::RunSystemOnce;
    use bevy_math::{
        Curve,
        curve::{Ease, FunctionCurve, Interval},
    };
    use bevy_replicon::prelude::*;
    use bevy_state::app::StatesPlugin;
    use bevy_time::{Time, Virtual};
    use core::time::Duration;
    use lightyear_core::history_buffer::HistoryState;
    use lightyear_core::prelude::{ConfirmedHistory, FrameInterpolationHistory};
    use lightyear_core::tick::Tick;
    use lightyear_interpolation::{
        plugin::InterpolationMarkerPlugin, registry::AppInterpolationExt, rules::InterpolationFns,
    };
    use lightyear_replication::diffable::Diffable as LightyearDiffable;
    use lightyear_replication::prelude::*;

    #[derive(Component, Clone, Debug, Default, PartialEq)]
    struct TestPos(f32);

    impl Ease for TestPos {
        fn interpolating_curve_unbounded(start: Self, end: Self) -> impl Curve<Self> {
            FunctionCurve::new(Interval::UNIT, move |t| {
                TestPos(start.0 + (end.0 - start.0) * t)
            })
        }
    }

    impl LightyearDiffable<TestPos> for TestPos {
        fn base_value() -> Self {
            Self::default()
        }

        fn diff(&self, new: &Self) -> TestPos {
            TestPos(new.0 - self.0)
        }

        fn apply_diff(&mut self, delta: &TestPos) {
            self.0 += delta.0;
        }
    }

    /// Second corrected type: the example blends up to four (Position,
    /// Rotation, velocities), and per-type fan-out needs its own coverage —
    /// the duplicate-marker panic proved single-type tests miss it.
    #[derive(Component, Clone, Debug, Default, PartialEq)]
    struct TestRot(f32);

    impl Ease for TestRot {
        fn interpolating_curve_unbounded(start: Self, end: Self) -> impl Curve<Self> {
            FunctionCurve::new(Interval::UNIT, move |t| {
                TestRot(start.0 + (end.0 - start.0) * t)
            })
        }
    }

    impl LightyearDiffable<TestRot> for TestRot {
        fn base_value() -> Self {
            Self::default()
        }

        fn diff(&self, new: &Self) -> TestRot {
            TestRot(new.0 - self.0)
        }

        fn apply_diff(&mut self, delta: &TestRot) {
            self.0 += delta.0;
        }
    }

    fn switch_app() -> App {
        let mut app = App::new();
        app.add_plugins((
            StatesPlugin,
            RepliconSharedPlugin {
                auth_method: AuthMethod::None,
            },
            PredictionMarkerPlugin,
            InterpolationMarkerPlugin,
        ));
        app.init_resource::<PredictionRegistry>();
        // Same resource the plugin installs via `add_timeline_switch_systems`.
        // Requests are components, so there is no message or cursor to register.
        app.init_resource::<TimelineSwitchSettings>();
        app.init_resource::<lightyear_core::prelude::LocalTimeline>();
        app.init_resource::<lightyear_sync::prelude::LocalTimelineSync>();
        app.insert_resource(Time::<Virtual>::default());
        app.component::<TestPos>().predict().add_correction();
        app.component::<TestRot>().predict().add_correction();
        app.interpolate_with::<TestPos>(InterpolationFns::no_history(|start, end, t| {
            TestPos(start.0 + (end.0 - start.0) * t)
        }));
        app.finish();
        app
    }

    /// Runs the `PostUpdate` pass that saves the on-screen value of the
    /// requests inserted since the last one.
    fn run_save(app: &mut App) {
        app.world_mut()
            .run_system_once(save_previous_visuals)
            .unwrap();
        app.world_mut().flush();
    }

    /// Runs the `PreUpdate` pass that applies the saved requests.
    fn run_apply(app: &mut App) {
        app.world_mut()
            .run_system_once(apply_saved_switches)
            .unwrap();
        app.world_mut().flush();
    }

    /// Runs the `PostUpdate` pass that applies the correction to the live value,
    /// which is what makes `live` hold the rendered value.
    fn run_correction_apply(app: &mut App) {
        use crate::correction::update_visual_correction;
        use bevy_ecs::system::RunSystemOnce;

        app.world_mut()
            .run_system_once(update_visual_correction::<TestPos, TestPos>)
            .unwrap();
        app.world_mut().flush();
    }

    /// Runs the `PostUpdate` pass that records corrections from the saved
    /// values: the shared system, which is also what a rollback goes through.
    fn run_blend(app: &mut App) {
        app.world_mut()
            .run_system_once(crate::correction::create_visual_corrections)
            .unwrap();
        app.world_mut().flush();
    }

    /// Inserts one request and runs the two passes, the way the schedule does
    /// over two frames: the value is saved at the end of the frame the request
    /// is seen, and the markers swap at the start of the next one.
    fn send_switch(app: &mut App, entity: Entity, request: TimelineSwitch) {
        app.world_mut().entity_mut(entity).insert(request);
        run_save(app);
        run_apply(app);
    }

    /// Shared blend window stamped on `entity` by the switch handler.
    fn blend_marker(world: &World, entity: Entity) -> SwitchBlend {
        *world.get::<SwitchBlend>(entity).expect("blend marker")
    }

    #[test]
    fn switch_to_predicted_swaps_markers_and_saves_visual() {
        let mut app = switch_app();
        let entity = app.world_mut().spawn((TestPos(10.0), Interpolated)).id();
        app.world_mut().flush();

        let request = TimelineSwitch::to_predicted().with_transition_secs(0.5);
        assert_eq!(request.direction(), Some(SwitchDirection::ToPredicted));
        assert_eq!(request.transition_override(), Some(0.5));
        send_switch(&mut app, entity, request);

        let world = app.world();
        assert!(world.get::<Predicted>(entity).is_some());
        assert!(world.get::<Interpolated>(entity).is_none());
        // A switch to prediction renders through frame interpolation even
        // though the renderer observer (On<Add, Position>) never re-fires when
        // the markers swap.
        assert!(world.get::<FrameInterpolate>(entity).is_some());
        // Nothing is saved in this direction: the forced rollback captures the
        // same value from the same live component before restoring it.
        assert!(world.get::<PreviousVisual<TestPos>>(entity).is_none());
        // The window is live and the error for this frame has not been written
        // yet: the blend pass runs later in the frame.
        assert!(world.get::<SwitchBlend>(entity).is_some());
        assert!(world.get::<VisualCorrection<TestPos>>(entity).is_none());
        assert_eq!(blend_marker(world, entity).total_secs(), 0.5);
    }

    #[test]
    fn switch_to_interpolated_swaps_markers_and_saves_visual() {
        let mut app = switch_app();
        let entity = app
            .world_mut()
            .spawn((TestPos(4.0), Predicted, FrameInterpolate))
            .id();
        app.world_mut().flush();

        send_switch(
            &mut app,
            entity,
            TimelineSwitch::to_interpolated().with_transition_secs(0.25),
        );

        let world = app.world();
        assert!(world.get::<Interpolated>(entity).is_some());
        assert!(world.get::<Predicted>(entity).is_none());
        // Delayed interpolation owns the pose from here on; frame
        // interpolation would freeze the render on its stale value.
        assert!(world.get::<FrameInterpolate>(entity).is_none());
        assert_eq!(
            world
                .get::<PreviousVisual<TestPos>>(entity)
                .map(|from| from.0.clone()),
            Some(TestPos(4.0))
        );
        assert!(world.get::<SwitchBlend>(entity).is_some());
    }

    /// The blend writes one error per frame: `(saved - destination)` scaled by
    /// how much of the window is left, so the rendered value is a lerp from the
    /// value on screen at the switch to whatever the destination has now. The
    /// live value is left alone; the shared apply adds the error afterwards.
    #[test]
    fn blend_waits_for_a_component_the_destination_has_not_presented() {
        let mut app = switch_app();
        let entity = app
            .world_mut()
            .spawn((TestPos(10.0), Predicted, FrameInterpolate))
            .id();
        app.world_mut().flush();

        send_switch(
            &mut app,
            entity,
            TimelineSwitch::to_interpolated().with_transition_secs(0.5),
        );
        app.world_mut().entity_mut(entity).remove::<TestPos>();
        app.world_mut().flush();
        assert!(
            app.world().get::<PreviousVisual<TestPos>>(entity).is_some(),
            "the saved value must outlive the component it was read from"
        );

        run_blend(&mut app);
        assert!(
            app.world()
                .get::<VisualCorrection<TestPos>>(entity)
                .is_none(),
            "nothing to blend without a destination value"
        );

        // A snapshot arrives with the delayed value; the gap is measured now.
        app.world_mut().entity_mut(entity).insert(TestPos(16.0));
        app.world_mut().flush();
        run_blend(&mut app);
        assert_eq!(
            app.world()
                .get::<VisualCorrection<TestPos>>(entity)
                .map(|c| c.error.clone()),
            Some(TestPos(-6.0))
        );
    }

    /// A blend's error decays on its window's curve, not on the entity's
    /// correction policy: the length and curve the caller asked for are what the
    /// transition looks like. An unbounded curve keeps that shape through the
    /// window too — its per-frame keep ratio is constant, so the window bounds it
    /// by its length rather than by normalising it.
    #[test]
    fn blend_error_decays_on_the_window_curve() {
        use crate::correction::update_visual_correction;
        use crate::manager::PredictionManager;
        use bevy_ecs::system::RunSystemOnce;

        let mut app = switch_app();
        app.insert_resource(PredictionManager::default());
        // Same offset, one with a window and one without. Early in a one second
        // smoothstep window the curve gives up almost nothing, while the policy's
        // 200 ms half-life is already shrinking it.
        let blended = app
            .world_mut()
            .spawn((
                TestPos(10.0),
                Predicted,
                SwitchBlend::new(0.0, 1.0, CorrectionEase::Smoothstep),
                VisualCorrection::new(TestPos(-1.0), 0.0),
            ))
            .id();
        let policy = app
            .world_mut()
            .spawn((
                TestPos(10.0),
                Predicted,
                VisualCorrection::new(TestPos(-1.0), 0.0),
            ))
            .id();
        // Slower than the policy's own exponential: the window and the policy
        // combine by taking whichever gives the error up more slowly, so only a
        // curve that is slower than the floor can be the one observed here. (A
        // faster one would be floored, and the assertion below could not tell the
        // two exponential periods apart.)
        let exponential = CorrectionEase::Exponential {
            decay_ratio: 0.5,
            decay_period_secs: 0.4,
        };
        let unbounded = app
            .world_mut()
            .spawn((
                TestPos(10.0),
                Predicted,
                SwitchBlend::new(0.0, 1.0, exponential),
                VisualCorrection::new(TestPos(-1.0), 0.0),
            ))
            .id();
        app.world_mut().flush();

        app.world_mut()
            .resource_mut::<Time<Virtual>>()
            .advance_by(Duration::from_millis(50));
        app.world_mut()
            .run_system_once(update_visual_correction::<TestPos, TestPos>)
            .unwrap();
        app.world_mut().flush();

        let error = |entity| {
            app.world()
                .get::<VisualCorrection<TestPos>>(entity)
                .map(|c| c.error.0)
                .unwrap()
        };
        let blend_error = error(blended);
        let policy_error = error(policy);
        assert!(
            blend_error < policy_error,
            "the window's curve should hold the error longer than the policy, \
             got blend {blend_error} and policy {policy_error}"
        );
        // The unbounded curve has no ramp to advance along, so one step of it is
        // exactly the fraction its own period gives up in 50 ms — and that is
        // visible here only because the window's period is slower than the
        // policy floor.
        assert!(
            (error(unbounded) - -exponential.remaining(0.05, 1.0)).abs() < 1e-5,
            "an unbounded window keeps its constant ratio, got {}",
            error(unbounded)
        );
    }

    /// The curve never gives an error up faster than the entity's own correction
    /// would. Its tail is steep, so without that floor a jump arriving late in the
    /// window would be dumped into a single frame.
    #[test]
    fn window_curve_never_outpaces_the_correction_policy() {
        use crate::correction::update_visual_correction;
        use crate::manager::PredictionManager;
        use bevy_ecs::system::RunSystemOnce;

        let mut app = switch_app();
        app.insert_resource(PredictionManager::default());
        let entity = app
            .world_mut()
            .spawn((
                TestPos(10.0),
                Predicted,
                // 99% through a half second window: the curve alone would drop
                // almost the whole error in this one frame.
                SwitchBlend::new(-0.479, 0.5, CorrectionEase::Smoothstep),
                VisualCorrection::new(TestPos(-10.0), 0.0),
            ))
            .id();
        app.world_mut().flush();
        app.world_mut()
            .resource_mut::<Time<Virtual>>()
            .advance_by(Duration::from_millis(16));

        app.world_mut()
            .run_system_once(update_visual_correction::<TestPos, TestPos>)
            .unwrap();
        app.world_mut().flush();

        let error = app
            .world()
            .get::<VisualCorrection<TestPos>>(entity)
            .map(|c| c.error.0)
            .unwrap();
        assert!(
            error < -9.0,
            "the policy floor should have kept most of the error, got {error}"
        );
    }

    /// Sets up the resources a rollback frame needs, with the rollback target at
    /// tick 5 and the local timeline at tick 10.
    fn rollback_env(app: &mut App) {
        use crate::manager::PredictionManager;
        use lightyear_core::prelude::LocalTimeline;
        use lightyear_core::timeline::Rollback;
        use lightyear_replication::registry::ComponentRegistry;

        app.init_resource::<LocalTimeline>();
        app.init_resource::<ComponentRegistry>();
        app.insert_resource(PredictionManager::default());
        app.insert_resource(Rollback::FromState);
        app.world_mut().resource_mut::<LocalTimeline>().0 = 10;
        app.world()
            .resource::<PredictionManager>()
            .set_rollback_tick(Tick(5));
    }

    /// History with a confirmed and a predicted value at `tick`.
    fn history_at(
        tick: u32,
        value: f32,
    ) -> (PredictionHistory<TestPos>, ConfirmedHistory<TestPos>) {
        use crate::predicted_history::PredictionHistory;

        let mut predicted = PredictionHistory::<TestPos>::default();
        predicted.add_predicted(Tick(tick), Some(TestPos(value)));
        let mut confirmed = ConfirmedHistory::<TestPos>::default();
        confirmed.insert(Tick(tick), HistoryState::Updated(TestPos(value)));
        (predicted, confirmed)
    }

    /// Runs one rollback frame with no switch involved: rewind, replay to
    /// `replayed`, and let the rollback path compute the correction.
    fn rollback_frame(app: &mut App, replayed: f32) {
        use crate::predicted_history::PredictionHistory;
        use crate::rollback::prepare_rollback;
        use bevy_ecs::system::RunSystemOnce;

        app.world_mut().run_system_once(prepare_rollback).unwrap();
        app.world_mut().flush();
        let entities: alloc::vec::Vec<Entity> = app
            .world_mut()
            .query_filtered::<Entity, With<PredictionHistory<TestPos>>>()
            .iter(app.world())
            .collect();
        for entity in entities {
            if let Some(mut position) = app.world_mut().get_mut::<TestPos>(entity) {
                position.0 = replayed;
            }
        }
        app.world_mut()
            .run_system_once(crate::correction::create_visual_corrections)
            .unwrap();
        app.world_mut().flush();
        // The blend pass is where a switch window computes its correction; it is
        // a no-op for entities without one.
        run_blend(app);
        run_correction_apply(app);
    }

    /// Switching to the interpolation timeline while a rollback correction is
    /// still decaying: the value to blend from has to be the render, which
    /// already carries that correction, and the new measurement replaces it.
    #[test]
    fn switch_to_interpolated_while_a_correction_is_in_progress() {
        use bevy_time::Fixed;

        let mut app = switch_app();
        app.insert_resource(Time::<Fixed>::from_duration(Duration::from_secs(1)));
        rollback_env(&mut app);
        let (history, confirmed) = history_at(5, 5.0);
        let entity = app
            .world_mut()
            .spawn((
                TestPos(10.0),
                Predicted,
                FrameInterpolate,
                history,
                confirmed,
                FrameInterpolationHistory::<TestPos> {
                    previous_value: Some(TestPos(5.0)),
                    current_value: Some(TestPos(10.0)),
                },
            ))
            .id();
        app.world_mut().flush();

        // A rollback puts a correction in flight: the render holds at 10 while
        // the simulation has moved to 20.
        rollback_frame(&mut app, 20.0);
        // The rollback holds the render somewhere between the pre-rollback
        // render and the replayed value; what matters here is that a correction
        // is in flight and that the switch measures from the render.
        let render = app.world().get::<TestPos>(entity).unwrap().0;
        let in_flight = app
            .world()
            .get::<VisualCorrection<TestPos>>(entity)
            .map(|c| c.error.0);
        assert!(
            in_flight.is_some_and(|error| error.abs() > 0.5),
            "the rollback should have left a correction, got {in_flight:?}"
        );

        // Now switch to the interpolation timeline.
        app.world_mut()
            .entity_mut(entity)
            .insert(TimelineSwitch::to_interpolated().with_transition_secs(0.5));
        run_save(&mut app);
        run_apply(&mut app);
        // The destination timeline writes its first value.
        app.world_mut().get_mut::<TestPos>(entity).unwrap().0 = 30.0;
        run_blend(&mut app);

        assert_eq!(
            app.world()
                .get::<VisualCorrection<TestPos>>(entity)
                .map(|c| c.error.clone()),
            Some(TestPos(render - 30.0)),
            "the switch must measure from the render it interrupted"
        );
    }

    /// The forced rollback asks for a rewind to the last processed confirmed
    /// tick. When the entity's confirmed history starts *after* that tick there
    /// is no state to restore, so the rollback captures nothing — and the switch
    /// to the prediction timeline is then the only thing that can record what to
    /// blend from.
    #[test]
    fn forced_rollback_that_finds_no_state_can_still_blend() {
        use bevy_time::Fixed;
        use lightyear_interpolation::registry::InterpolationRegistry;

        let mut app = switch_app();
        app.init_resource::<InterpolationRegistry>();
        app.insert_resource(Time::<Fixed>::from_duration(Duration::from_secs(1)));
        rollback_env(&mut app);
        // Confirmed data at tick 8 only: the rollback target is tick 5.
        let (history, confirmed) = history_at(8, 8.0);
        let entity = app
            .world_mut()
            .spawn((TestPos(10.0), Interpolated, history, confirmed))
            .id();
        app.world_mut().flush();

        app.world_mut()
            .entity_mut(entity)
            .insert(TimelineSwitch::to_predicted().with_transition_secs(0.5));
        run_save(&mut app);
        run_apply(&mut app);
        rollback_frame(&mut app, 30.0);

        let error = app
            .world()
            .get::<VisualCorrection<TestPos>>(entity)
            .map(|c| c.error.clone());
        assert_eq!(
            error,
            Some(TestPos(10.0 - 30.0)),
            "a switch must still blend when the rollback finds no state to restore"
        );
    }

    /// A switch to the prediction timeline does not save anything: the forced
    /// rollback is the capture, and it takes the value from the same live
    /// component before restoring it. This pins that the rollback really is
    /// enough, since nothing else records the value in this direction.
    #[test]
    fn forced_rollback_captures_the_value_a_switch_blends_from() {
        use crate::manager::PredictionManager;
        use crate::predicted_history::PredictionHistory;
        use crate::rollback::prepare_rollback;
        use bevy_ecs::system::RunSystemOnce;
        use lightyear_core::prelude::LocalTimeline;
        use lightyear_core::timeline::Rollback;
        use lightyear_replication::registry::ComponentRegistry;

        let mut app = switch_app();
        app.init_resource::<LocalTimeline>();
        app.init_resource::<ComponentRegistry>();
        app.insert_resource(PredictionManager::default());
        app.insert_resource(Rollback::FromState);
        app.world_mut().resource_mut::<LocalTimeline>().0 = 10;
        app.world()
            .resource::<PredictionManager>()
            .set_rollback_tick(Tick(5));
        let mut history = PredictionHistory::<TestPos>::default();
        history.add_predicted(Tick(5), Some(TestPos(5.0)));
        let mut confirmed = ConfirmedHistory::<TestPos>::default();
        confirmed.insert(Tick(5), HistoryState::Updated(TestPos(5.0)));
        // Interpolated and showing 10.
        let entity = app
            .world_mut()
            .spawn((TestPos(10.0), Interpolated, history, confirmed))
            .id();
        app.world_mut().flush();

        // Frame F: the switch saves nothing in this direction.
        app.world_mut()
            .entity_mut(entity)
            .insert(TimelineSwitch::to_predicted().with_transition_secs(0.5));
        run_save(&mut app);
        assert!(app.world().get::<PreviousVisual<TestPos>>(entity).is_none());

        // Frame F+1: the markers swap, then the rollback captures.
        run_apply(&mut app);
        app.world_mut().run_system_once(prepare_rollback).unwrap();
        app.world_mut().flush();

        // The rollback is the capture in this direction, and it takes the value
        // the entity was rendering.
        assert_eq!(
            app.world()
                .get::<PreviousVisual<TestPos>>(entity)
                .map(|p| p.0.clone()),
            Some(TestPos(10.0)),
            "the rollback must capture the value the switch blends from"
        );
        assert_eq!(
            app.world().get::<TestPos>(entity),
            Some(&TestPos(5.0)),
            "... while it rewinds the live value to the confirmed state"
        );
    }

    /// Drives a whole frame the way the schedule does around a switch to the
    /// prediction timeline: the rollback in `PreUpdate`, replay, then the switch
    /// correction pass in `PostUpdate`.
    fn frame_with_rollback(app: &mut App, replayed: f32) {
        use crate::manager::PredictionManager;
        use crate::predicted_history::PredictionHistory;
        use crate::rollback::prepare_rollback;
        use bevy_ecs::system::RunSystemOnce;
        use lightyear_core::timeline::Rollback;
        use lightyear_replication::registry::ComponentRegistry;

        app.world_mut().init_resource::<ComponentRegistry>();
        app.world_mut().init_resource::<PredictionManager>();
        app.world_mut().insert_resource(Rollback::FromState);
        app.world_mut()
            .resource_mut::<lightyear_core::prelude::LocalTimeline>()
            .0 = 10;
        app.world()
            .resource::<PredictionManager>()
            .set_rollback_tick(Tick(5));
        app.world_mut().flush();

        app.world_mut().run_system_once(prepare_rollback).unwrap();
        app.world_mut().flush();
        // Replay steps the simulation forward to the replayed value.
        let entities: alloc::vec::Vec<Entity> = app
            .world_mut()
            .query_filtered::<Entity, With<PredictionHistory<TestPos>>>()
            .iter(app.world())
            .collect();
        for entity in entities {
            if let Some(mut position) = app.world_mut().get_mut::<TestPos>(entity) {
                position.0 = replayed;
            }
        }
        app.world_mut()
            .run_system_once(crate::correction::create_visual_corrections)
            .unwrap();
        app.world_mut().flush();
        run_blend(app);
        // The apply then adds the correction to the live value, so `live` is the
        // render from here on.
        run_correction_apply(app);
    }

    /// A rollback during a switch to the prediction timeline must be measured
    /// against the render it interrupted, like any other rollback. The window is
    /// still open, so the rollback path leaves the correction to the blend; the
    /// saved value it writes is what the blend's next computation reads.
    #[test]
    fn rollback_during_a_to_predicted_blend_updates_the_correction() {
        use crate::predicted_history::PredictionHistory;
        use bevy_time::Fixed;
        use lightyear_core::prelude::LocalTimeline;
        use lightyear_interpolation::registry::InterpolationRegistry;

        let mut app = switch_app();
        app.init_resource::<InterpolationRegistry>();
        app.init_resource::<LocalTimeline>();
        app.insert_resource(Time::<Fixed>::from_duration(Duration::from_secs(1)));
        // Interpolated, showing 10, with history at the tick the rollbacks will
        // rewind to.
        let mut history = PredictionHistory::<TestPos>::default();
        history.add_predicted(Tick(5), Some(TestPos(5.0)));
        let mut confirmed = ConfirmedHistory::<TestPos>::default();
        confirmed.insert(Tick(5), HistoryState::Updated(TestPos(5.0)));
        let entity = app
            .world_mut()
            .spawn((TestPos(10.0), Interpolated, history, confirmed))
            .id();
        app.world_mut().flush();

        // Frame F: save the value on screen. F+1: swap the markers, roll back,
        // and let the blend compute its correction.
        app.world_mut()
            .entity_mut(entity)
            .insert(TimelineSwitch::to_predicted().with_transition_secs(0.5));
        run_save(&mut app);
        run_apply(&mut app);
        frame_with_rollback(&mut app, 30.0);

        // The apply bakes the correction into the live value, so from here the
        // live value is what renders, and the correction is the offset still to
        // be given up against the destination.
        let held = app.world().get::<TestPos>(entity).unwrap().0;
        assert_eq!(
            held, 10.0,
            "the switch frame should hold the render at the saved value"
        );

        // Mid-window: another rollback moves the destination again. The blend has
        // to re-measure from the render it interrupted, not keep the old offset.
        frame_with_rollback(&mut app, 100.0);

        let error = app
            .world()
            .get::<VisualCorrection<TestPos>>(entity)
            .map(|c| c.error.0)
            .unwrap();
        let render = app.world().get::<TestPos>(entity).unwrap().0;
        assert_eq!(
            render, held,
            "the render must stay where the blend held it (correction now {error})"
        );
        assert!(
            error < -70.0,
            "the correction must be re-measured against the new destination, got {error}"
        );
    }

    /// A switch takes over both the offset and the schedule of a correction that
    /// was in flight when it was applied: the value on screen becomes the blend's
    /// starting point, and the window's length governs from then on, whatever
    /// tuning the old correction had.
    #[test]
    fn switch_takes_over_an_in_flight_correction() {
        use crate::correction::update_visual_correction;
        use crate::manager::PredictionManager;
        use bevy_ecs::system::RunSystemOnce;

        let mut app = switch_app();
        app.insert_resource(PredictionManager::default());
        // Mid-correction: the destination timeline has written 12 and an error of
        // -2 from an earlier rollback is being decayed, so 10 is on screen.
        let entity = app
            .world_mut()
            .spawn((
                TestPos(12.0),
                Predicted,
                FrameInterpolate,
                VisualCorrection::new(TestPos(-2.0), 0.0),
            ))
            .id();
        app.world_mut().flush();
        app.world_mut()
            .resource_mut::<Time<Virtual>>()
            .advance_by(Duration::from_millis(16));
        app.world_mut()
            .run_system_once(update_visual_correction::<TestPos, TestPos>)
            .unwrap();
        app.world_mut().flush();
        // What the entity is showing now, error included.
        let on_screen = app.world().get::<TestPos>(entity).unwrap().0;

        app.world_mut()
            .entity_mut(entity)
            .insert(TimelineSwitch::to_interpolated().with_transition_secs(0.5));
        run_save(&mut app);
        run_apply(&mut app);
        app.world_mut().get_mut::<TestPos>(entity).unwrap().0 = 16.0;
        run_blend(&mut app);

        let world = app.world();
        // The offset is measured from what was on screen, and the window owns it
        // now: the old correction's tuning no longer applies.
        assert_eq!(
            world
                .get::<VisualCorrection<TestPos>>(entity)
                .map(|c| c.error.clone()),
            Some(TestPos(on_screen - 16.0))
        );
        assert!(
            on_screen > 10.0 && on_screen < 12.0,
            "the old correction was partway through decaying, got {on_screen}"
        );
        // The saved value was consumed: the window owns the error now.
        assert!(world.get::<PreviousVisual<TestPos>>(entity).is_none());
        assert!(world.get::<SwitchBlend>(entity).is_some());
    }

    /// The switch records one jump — the gap between the value on screen and
    /// the destination's value — and the ordinary decay gives it up. Same shape
    /// as a rollback correction: measure once, then shrink.
    #[test]
    fn blend_records_one_jump_and_decays_it() {
        let mut app = switch_app();
        let entity = app
            .world_mut()
            .spawn((TestPos(10.0), Predicted, FrameInterpolate))
            .id();
        app.world_mut().flush();

        send_switch(
            &mut app,
            entity,
            TimelineSwitch::to_interpolated().with_transition_secs(0.5),
        );
        // The destination timeline writes its first value between the apply and
        // the seed.
        app.world_mut().get_mut::<TestPos>(entity).unwrap().0 = 16.0;
        run_blend(&mut app);

        // error = saved - destination, so the render holds at the saved value.
        assert_eq!(
            app.world()
                .get::<VisualCorrection<TestPos>>(entity)
                .map(|c| c.error.clone()),
            Some(TestPos(-6.0))
        );

        // A second seed pass must not record the jump again: the offset is
        // already there and belongs to the window now.
        app.world_mut().get_mut::<TestPos>(entity).unwrap().0 = 20.0;
        run_blend(&mut app);
        assert_eq!(
            app.world()
                .get::<VisualCorrection<TestPos>>(entity)
                .map(|c| c.error.clone()),
            Some(TestPos(-6.0)),
            "the blend seeds once, it does not re-measure every frame"
        );
    }

    /// A rollback that lands while the window is open keeps its own hold, and
    /// the blend decays that hold instead of re-deriving an offset from the saved
    /// value. Re-deriving is what leaks: the offset would be scaled by how much
    /// of the window is left, which is nearly zero late in the window, so the
    #[test]
    fn switch_to_interpolated_saves_one_value_per_corrected_type() {
        let mut app = switch_app();
        let entity = app
            .world_mut()
            .spawn((TestPos(4.0), TestRot(1.0), Predicted, FrameInterpolate))
            .id();
        app.world_mut().flush();

        send_switch(&mut app, entity, TimelineSwitch::to_interpolated());

        let world = app.world();
        assert_eq!(
            world
                .get::<PreviousVisual<TestPos>>(entity)
                .map(|from| from.0.clone()),
            Some(TestPos(4.0))
        );
        assert_eq!(
            world
                .get::<PreviousVisual<TestRot>>(entity)
                .map(|from| from.0.clone()),
            Some(TestRot(1.0))
        );

        // Both destinations write, and both errors follow.
        app.world_mut().get_mut::<TestPos>(entity).unwrap().0 = 10.0;
        app.world_mut().get_mut::<TestRot>(entity).unwrap().0 = 3.0;
        run_blend(&mut app);
        assert_eq!(
            app.world()
                .get::<VisualCorrection<TestPos>>(entity)
                .map(|c| c.error.clone()),
            Some(TestPos(-6.0))
        );
        assert_eq!(
            app.world()
                .get::<VisualCorrection<TestRot>>(entity)
                .map(|c| c.error.clone()),
            Some(TestRot(-2.0))
        );
    }

    /// The frame history belongs to the era the entity is leaving, and neither
    /// direction can keep it: while interpolated, frame interpolation is not
    /// recording it (it only runs for archetypes with `FrameInterpolate`), and
    /// once interpolated, frame interpolation stops reading it. Keeping a stale
    /// one would let the next era restore it over the live value.
    #[test]
    fn switch_drops_the_frame_history_in_both_directions() {
        // To the prediction timeline: the fossil from the interpolated era goes.
        let mut app = switch_app();
        let entity = app.world_mut().spawn((TestPos(10.0), Interpolated)).id();
        app.world_mut()
            .entity_mut(entity)
            .insert(FrameInterpolationHistory::<TestPos> {
                previous_value: Some(TestPos(47.0)),
                current_value: Some(TestPos(47.7)),
            });
        app.world_mut().flush();
        send_switch(&mut app, entity, TimelineSwitch::to_predicted());
        assert!(
            app.world()
                .get::<FrameInterpolationHistory<TestPos>>(entity)
                .is_none(),
            "a switch to prediction must drop the stale history"
        );

        // To the interpolation timeline: frame interpolation stops reading it,
        // so a history that only describes the predicted era goes too.
        let mut app = switch_app();
        let entity = app
            .world_mut()
            .spawn((TestPos(10.0), Predicted, FrameInterpolate))
            .id();
        app.world_mut()
            .entity_mut(entity)
            .insert(FrameInterpolationHistory::<TestPos> {
                previous_value: Some(TestPos(4.0)),
                current_value: Some(TestPos(10.0)),
            });
        app.world_mut().flush();
        send_switch(&mut app, entity, TimelineSwitch::to_interpolated());
        assert!(
            app.world()
                .get::<FrameInterpolationHistory<TestPos>>(entity)
                .is_none(),
            "a switch to interpolation must not leave the predicted era's history"
        );
    }

    #[test]
    fn snap_duration_flips_without_blend_state() {
        for duration in [0.0, -1.0, f32::NAN] {
            let mut app = switch_app();
            let entity = app.world_mut().spawn((TestPos(1.0), Interpolated)).id();
            app.world_mut().flush();

            send_switch(
                &mut app,
                entity,
                TimelineSwitch::to_predicted().with_transition_secs(duration),
            );

            let world = app.world();
            assert!(world.get::<Predicted>(entity).is_some());
            assert!(world.get::<Interpolated>(entity).is_none());
            assert!(world.get::<PreviousVisual<TestPos>>(entity).is_none());
            assert!(world.get::<SwitchBlend>(entity).is_none());
        }
    }

    /// The contract every window curve is used through: whole at the start,
    /// converged at the end, never growing in between. A curve ending above zero
    /// would leave a permanent offset behind, and one that grew would make the
    /// render move away from its destination.
    #[test]
    fn ease_remaining_bookends_and_monotonic() {
        // Bounded curves are normalised over the window they run in.
        for ease in CorrectionEase::ALL {
            assert_eq!(ease.remaining(0.0, 1.0), 1.0, "{ease:?} starts whole");
            assert_eq!(ease.remaining(1.0, 1.0), 0.0, "{ease:?} ends converged");
            let mut prev = f32::INFINITY;
            let mut p = 0.0;
            while p <= 1.0 {
                let r = ease.remaining(p, 1.0);
                assert!(r <= prev, "{ease:?} must not grow at {p}");
                prev = r;
                p += 0.05;
            }
        }
    }

    /// The request's own overrides win; what is left resolves against
    /// [`TimelineSwitchSettings`] when the request is saved, so a game retunes the
    /// feel in one place and can still tune a single switch.
    #[test]
    fn settings_resolve_default_and_override() {
        let mut app = switch_app();
        app.world_mut().insert_resource(TimelineSwitchSettings {
            default_transition_secs: 2.0,
            default_ease: CorrectionEase::Smoothstep,
        });
        let bare = app.world_mut().spawn((TestPos(1.0), Interpolated)).id();
        let overridden = app.world_mut().spawn((TestPos(2.0), Interpolated)).id();
        app.world_mut().flush();

        send_switch(&mut app, bare, TimelineSwitch::to_predicted());
        send_switch(
            &mut app,
            overridden,
            TimelineSwitch::to_predicted()
                .with_transition_secs(0.5)
                .with_ease(CorrectionEase::EaseOutCubic),
        );

        let world = app.world();
        assert_eq!(blend_marker(world, bare).total_secs(), 2.0);
        assert_eq!(blend_marker(world, bare).ease(), CorrectionEase::Smoothstep);
        assert_eq!(blend_marker(world, overridden).total_secs(), 0.5);
        assert_eq!(
            blend_marker(world, overridden).ease(),
            CorrectionEase::EaseOutCubic
        );
    }

    /// The exponential is the unbounded curve: it never reaches zero, its keep
    /// ratio is constant (so it does not depend on where in the curve we are),
    /// and it matches the correction policy's own lerp ratio.
    #[test]
    fn exponential_ease_matches_the_correction_policy() {
        let policy = crate::correction::CorrectionPolicy::new(0.5, Duration::from_millis(200));
        let ease = policy.ease();
        assert!(ease.is_unbounded());
        for dt in [
            Duration::from_millis(8),
            Duration::from_millis(16),
            Duration::from_millis(50),
        ] {
            assert!(
                (ease.keep(0.0, dt.as_secs_f32(), 1.0) - policy.lerp_ratio(0.0, dt)).abs() < 1e-6,
                "the ease must reproduce the policy's own ratio"
            );
        }
        assert!(
            ease.remaining(10.0, 1.0) > 0.0,
            "an exponential never reaches zero"
        );
        // Half a decay period in, half of it is left.
        assert!((ease.remaining(0.2, 1.0) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn invalid_requests_are_dropped() {
        let mut app = switch_app();
        let predicted = app.world_mut().spawn((TestPos(1.0), Predicted)).id();
        let interpolated = app.world_mut().spawn((TestPos(2.0), Interpolated)).id();
        let plain = app.world_mut().spawn(TestPos(3.0)).id();
        app.world_mut().flush();

        // Wrong current mode (and a mode-less entity): nothing switches, no
        // blend.
        send_switch(
            &mut app,
            predicted,
            TimelineSwitch::to_predicted().with_transition_secs(0.5),
        );
        send_switch(
            &mut app,
            interpolated,
            TimelineSwitch::to_interpolated().with_transition_secs(0.5),
        );
        send_switch(
            &mut app,
            plain,
            TimelineSwitch::to_predicted().with_transition_secs(0.5),
        );
        // A flip has nothing to flip from there either: it resolves against the
        // entity's markers, and there are none.
        send_switch(&mut app, plain, TimelineSwitch::default());

        let world = app.world();
        assert!(world.get::<Predicted>(predicted).is_some());
        assert!(world.get::<SwitchBlend>(predicted).is_none());
        assert!(world.get::<Interpolated>(interpolated).is_some());
        assert!(world.get::<SwitchBlend>(interpolated).is_none());
        assert!(world.get::<SwitchBlend>(plain).is_none());
        // A dropped request does not stay on the entity and fire later.
        assert!(world.get::<TimelineSwitch>(plain).is_none());

        // A second request while blending is ignored.
        send_switch(
            &mut app,
            interpolated,
            TimelineSwitch::to_predicted().with_transition_secs(0.5),
        );
        {
            let world = app.world();
            assert!(world.get::<Predicted>(interpolated).is_some());
            assert!(world.get::<SwitchBlend>(interpolated).is_some());
        }
        send_switch(
            &mut app,
            interpolated,
            TimelineSwitch::to_interpolated().with_transition_secs(0.5),
        );
        let world = app.world();
        assert!(world.get::<Predicted>(interpolated).is_some());
        assert!(world.get::<Interpolated>(interpolated).is_none());
        assert!(world.get::<SwitchBlend>(interpolated).is_some());
    }

    /// A bare `TimelineSwitch::default()` carries no direction: the save pass
    /// picks the timeline the entity is not on, so the same value works in both
    /// directions and callers that only care about toggling never name one.
    #[test]
    fn default_flips_the_entity_to_the_other_timeline() {
        let mut app = switch_app();
        let to_predicted = app.world_mut().spawn((TestPos(1.0), Interpolated)).id();
        let to_interpolated = app
            .world_mut()
            .spawn((TestPos(2.0), Predicted, FrameInterpolate))
            .id();
        app.world_mut().flush();

        assert_eq!(TimelineSwitch::default().direction(), None);
        send_switch(
            &mut app,
            to_predicted,
            TimelineSwitch::default().with_transition_secs(0.5),
        );
        send_switch(
            &mut app,
            to_interpolated,
            TimelineSwitch::default().with_transition_secs(0.5),
        );

        let world = app.world();
        assert!(world.get::<Predicted>(to_predicted).is_some());
        assert!(world.get::<Interpolated>(to_predicted).is_none());
        assert!(world.get::<Interpolated>(to_interpolated).is_some());
        assert!(world.get::<Predicted>(to_interpolated).is_none());
        // Both flips blend, so both entities carry the window the save pass
        // resolved from the override.
        assert_eq!(blend_marker(world, to_predicted).total_secs(), 0.5);
        assert_eq!(blend_marker(world, to_interpolated).total_secs(), 0.5);
    }

    #[test]
    fn expired_switch_window_clears_state_of_missing_component() {
        use bevy_ecs::system::RunSystemOnce;

        let mut app = switch_app();
        let entity = app.world_mut().spawn((TestPos(10.0), Predicted)).id();
        app.world_mut().flush();
        send_switch(
            &mut app,
            entity,
            TimelineSwitch::to_interpolated().with_transition_secs(0.5),
        );
        app.world_mut().entity_mut(entity).remove::<TestPos>();
        app.world_mut().flush();

        app.world_mut()
            .resource_mut::<Time<Virtual>>()
            .advance_by(Duration::from_millis(600));
        app.world_mut()
            .run_system_once(expire_switch_blends)
            .unwrap();
        app.world_mut().flush();

        let world = app.world();
        assert!(world.get::<SwitchBlend>(entity).is_none());
        assert!(world.get::<PreviousVisual<TestPos>>(entity).is_none());
        assert!(world.get::<VisualCorrection<TestPos>>(entity).is_none());
    }

    #[test]
    fn prepare_skips_visual_capture_on_interpolated() {
        use crate::manager::PredictionManager;
        use crate::predicted_history::PredictionHistory;
        use crate::rollback::prepare_rollback;
        use bevy_ecs::system::RunSystemOnce;
        use lightyear_core::timeline::Rollback;
        use lightyear_replication::registry::ComponentRegistry;

        let mut app = switch_app();
        app.world_mut().init_resource::<ComponentRegistry>();
        app.world_mut()
            .insert_resource(PredictionManager::default());
        app.world_mut().insert_resource(Rollback::FromState);
        app.world_mut()
            .resource_mut::<lightyear_core::prelude::LocalTimeline>()
            .0 = 10;
        app.world()
            .resource::<PredictionManager>()
            .set_rollback_tick(Tick(5));

        fn histories(value: f32) -> (PredictionHistory<TestPos>, ConfirmedHistory<TestPos>) {
            let mut predicted = PredictionHistory::<TestPos>::default();
            predicted.add_predicted(Tick(5), Some(TestPos(value)));
            let mut confirmed = ConfirmedHistory::<TestPos>::default();
            confirmed.insert(Tick(5), HistoryState::Updated(TestPos(value)));
            (predicted, confirmed)
        }
        let (predicted_history, predicted_confirmed) = histories(5.0);
        let predicted = app
            .world_mut()
            .spawn((
                TestPos(10.0),
                Predicted,
                predicted_history,
                predicted_confirmed,
            ))
            .id();
        let (interp_history, interp_confirmed) = histories(5.0);
        let interp = app
            .world_mut()
            .spawn((
                TestPos(10.0),
                Interpolated,
                interp_history,
                interp_confirmed,
            ))
            .id();
        app.world_mut().flush();
        app.world_mut().run_system_once(prepare_rollback).unwrap();

        let world = app.world();
        // Both rewind to the rollback target ...
        assert_eq!(world.get::<TestPos>(predicted), Some(&TestPos(5.0)));
        assert_eq!(world.get::<TestPos>(interp), Some(&TestPos(5.0)));
        // ... but only the predicted one captures for correction.
        assert_eq!(
            world
                .get::<PreviousVisual<TestPos>>(predicted)
                .map(|p| p.0.clone()),
            Some(TestPos(10.0))
        );
        assert!(world.get::<PreviousVisual<TestPos>>(interp).is_none());
    }

    /// Which switches ask for a forced rollback. A switch to the prediction
    /// timeline needs one: the world rewinds to the scanned frontier and replays
    /// under the new marker, and that rollback is also the capture the blend
    /// starts from. The other direction just presents the delayed value, and
    /// before the first sync there is no scanned tick to rewind to, so the switch
    /// applies without one.
    #[test]
    fn forced_rollback_is_requested_only_for_to_predicted_with_a_scanned_tick() {
        let k = Tick(90);
        for (direction, scanned, expect_forced) in [
            (SwitchDirection::ToPredicted, Some(k), Some(k)),
            (SwitchDirection::ToInterpolated, Some(k), None),
            (SwitchDirection::ToPredicted, None, None),
        ] {
            let mut app = switch_app();
            app.init_resource::<StateRollbackMetadata>();
            if let Some(tick) = scanned {
                app.world_mut()
                    .resource_mut::<StateRollbackMetadata>()
                    .set_last_processed_confirmed_tick(tick);
            }
            let entity = match direction {
                SwitchDirection::ToPredicted => {
                    app.world_mut().spawn((TestPos(10.0), Interpolated)).id()
                }
                SwitchDirection::ToInterpolated => app
                    .world_mut()
                    .spawn((TestPos(10.0), Predicted, FrameInterpolate))
                    .id(),
            };
            app.world_mut().flush();

            send_switch(
                &mut app,
                entity,
                match direction {
                    SwitchDirection::ToPredicted => {
                        TimelineSwitch::to_predicted().with_transition_secs(0.5)
                    }
                    SwitchDirection::ToInterpolated => {
                        TimelineSwitch::to_interpolated().with_transition_secs(0.5)
                    }
                },
            );

            let world = app.world();
            assert_eq!(
                world.get::<Predicted>(entity).is_some(),
                direction == SwitchDirection::ToPredicted,
                "{direction:?} must swap the markers"
            );
            assert!(world.get::<SwitchBlend>(entity).is_some());
            assert_eq!(
                world
                    .resource::<StateRollbackMetadata>()
                    .forced_rollback_tick(),
                expect_forced,
                "{direction:?} with scanned tick {scanned:?}"
            );
        }
    }

    /// A switch to the prediction timeline has nothing saved by the switch
    /// itself: the rollback it asks for is what captures the value to blend
    /// from. So when no rollback runs — before the first sync, where there is no
    /// confirmed tick to rewind to — there is nothing to measure, and the switch
    /// takes effect without a blend. Nothing is left behind either.
    #[test]
    fn to_predicted_without_a_rollback_has_nothing_to_blend_from() {
        let mut app = switch_app();
        app.init_resource::<StateRollbackMetadata>();
        let entity = app.world_mut().spawn((TestPos(10.0), Interpolated)).id();
        app.world_mut().flush();

        app.world_mut()
            .entity_mut(entity)
            .insert(TimelineSwitch::to_predicted().with_transition_secs(0.5));
        run_save(&mut app);
        assert!(
            app.world().get::<PreviousVisual<TestPos>>(entity).is_none(),
            "the switch saves nothing in this direction"
        );

        run_apply(&mut app);
        app.world_mut().get_mut::<TestPos>(entity).unwrap().0 = 30.0;
        run_blend(&mut app);

        let world = app.world();
        assert!(
            world.get::<PreviousVisual<TestPos>>(entity).is_none(),
            "nothing was captured, because no rollback ran"
        );
        assert!(
            world.get::<VisualCorrection<TestPos>>(entity).is_none(),
            "no measurement means no correction: the switch is not blended"
        );
        // The window still runs its length, so the entity is not re-switched
        // while it is nominally blending.
        assert!(world.get::<SwitchBlend>(entity).is_some());
    }

    /// A committed switch owns the entity until it is applied: the save pass has
    /// already recorded the value on screen for it, so a request arriving inside
    /// that window must be dropped rather than resolved on top of it. Resolving
    /// it would leave the recorded value to be measured as a jump that never
    /// happened — the entity would be pinned back to the value from a frame the
    /// replacement has nothing to do with — and would apply the replacement
    /// against markers it never saw.
    #[test]
    fn requests_arriving_while_a_switch_is_committed_are_dropped() {
        let mut app = switch_app();
        let entity = app.world_mut().spawn((TestPos(10.0), Predicted)).id();
        app.world_mut().flush();

        // Frame F: the request is committed, and its value on screen recorded.
        app.world_mut()
            .entity_mut(entity)
            .insert(TimelineSwitch::to_interpolated().with_transition_secs(0.5));
        run_save(&mut app);
        {
            let world = app.world();
            assert!(world.get::<PendingSwitch>(entity).is_some());
            assert!(world.get::<TimelineSwitch>(entity).is_none());
            assert!(world.get::<PreviousVisual<TestPos>>(entity).is_some());
        }

        // A replacement arrives before the apply pass, with every field set so
        // its values cannot be told apart from a resolved request's. A second
        // save pass — which is what the window is at risk from, since the
        // recorded value is only meaningful for the switch it was taken for —
        // drops it and leaves the committed switch alone.
        app.world_mut().entity_mut(entity).insert(
            TimelineSwitch::to_interpolated()
                .with_transition_secs(0.25)
                .with_ease(CorrectionEase::Linear),
        );
        run_save(&mut app);
        {
            let world = app.world();
            assert!(
                world.get::<TimelineSwitch>(entity).is_none(),
                "a request that arrives while one is committed must not linger"
            );
            let committed = world
                .get::<PendingSwitch>(entity)
                .expect("committed switch");
            assert_eq!(
                committed.total_secs, 0.5,
                "the committed switch is untouched"
            );
            assert_eq!(committed.ease, CorrectionEase::default());
        }

        // The committed switch is the one that applies, with the blend it was
        // committed with.
        run_apply(&mut app);
        let world = app.world();
        assert!(world.get::<Interpolated>(entity).is_some());
        assert!(world.get::<Predicted>(entity).is_none());
        assert!(world.get::<PendingSwitch>(entity).is_none());
        assert_eq!(blend_marker(world, entity).total_secs(), 0.5);

        // Nothing is left over to be applied later.
        run_save(&mut app);
        run_apply(&mut app);
        let world = app.world();
        assert!(world.get::<Interpolated>(entity).is_some());
        assert_eq!(blend_marker(world, entity).total_secs(), 0.5);
    }

    /// A request is a component, so two requests for the same entity in one
    /// frame are two inserts: the second replaces the first. Only the request
    /// that is left is served, and it is served against the timeline the entity
    /// is actually on when the save pass runs.
    #[test]
    fn contradictory_requests_in_one_frame_keep_the_last_insert() {
        for (first, second, expect_predicted, expected_reason) in [
            // Up first, then down: the request left on the entity wants to
            // leave the prediction timeline, which an interpolated entity is
            // not on, so it is dropped and nothing changes.
            (
                SwitchDirection::ToPredicted,
                SwitchDirection::ToInterpolated,
                false,
                "the last insert is invalid on an interpolated entity",
            ),
            // Down first, then up: the request left is the valid one, so the
            // entity moves to the prediction timeline.
            (
                SwitchDirection::ToInterpolated,
                SwitchDirection::ToPredicted,
                true,
                "the last insert is valid on an interpolated entity",
            ),
        ] {
            let mut app = switch_app();
            let entity = app.world_mut().spawn((TestPos(1.0), Interpolated)).id();
            app.world_mut().flush();

            let request = |direction| match direction {
                SwitchDirection::ToPredicted => TimelineSwitch::to_predicted(),
                SwitchDirection::ToInterpolated => TimelineSwitch::to_interpolated(),
            };
            app.world_mut()
                .entity_mut(entity)
                .insert(request(first))
                .insert(request(second));
            run_save(&mut app);
            run_apply(&mut app);

            let world = app.world();
            assert_eq!(
                world.get::<Predicted>(entity).is_some(),
                expect_predicted,
                "{second:?} inserted after {first:?}: {expected_reason}"
            );
        }

        // Before the save pass commits, the last insert is the request that
        // sticks, including its blend length.
        let mut app = switch_app();
        let entity = app.world_mut().spawn((TestPos(1.0), Interpolated)).id();
        app.world_mut().flush();
        app.world_mut()
            .entity_mut(entity)
            .insert(TimelineSwitch::to_predicted().with_transition_secs(0.5))
            .insert(TimelineSwitch::to_predicted().with_transition_secs(0.25));
        run_save(&mut app);
        run_apply(&mut app);
        let world = app.world();
        assert!(world.get::<Predicted>(entity).is_some());
        assert_eq!(blend_marker(world, entity).total_secs(), 0.25);
    }

    #[test]
    fn consumed_request_does_not_replay() {
        // One-shot semantics: running a handler with no request on the entity
        // changes nothing, even right after a switch.
        let mut app = switch_app();
        let entity = app.world_mut().spawn((TestPos(1.0), Interpolated)).id();
        app.world_mut().flush();

        send_switch(&mut app, entity, TimelineSwitch::to_predicted());
        assert!(app.world().get::<Predicted>(entity).is_some());
        // The request went with the markers it swapped.
        assert!(app.world().get::<TimelineSwitch>(entity).is_none());
        // No request left: both passes are noops.
        run_save(&mut app);
        run_apply(&mut app);
        assert!(app.world().get::<Predicted>(entity).is_some());
        assert!(app.world().get::<Interpolated>(entity).is_none());
    }
}
