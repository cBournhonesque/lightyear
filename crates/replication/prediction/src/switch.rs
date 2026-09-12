//! Client-side switching between the prediction and interpolation timelines.
//!
//! Gameplay code (proximity, pickup ownership, ...) sends a [`TimelineSwitch`]
//! message to move one entity between the prediction timeline (client-ahead
//! simulation) and the interpolation timeline (delayed server presentation).
//!
//! # Order of operations
//!
//! A request is handled over two frames. Say the request is sent on frame F
//! (gameplay code usually sends it from `Update`):
//!
//! 1. `save_previous_visuals` runs in `PostUpdate` of frame F, after frame
//!    interpolation and after the correction apply, so what it reads is what the
//!    frame shows. It saves that value as
//!    [`PreviousVisual`](crate::correction::PreviousVisual) — for a switch to the
//!    interpolation timeline, the only thing that can record it — and carries the
//!    request to the next frame as `PendingSwitch`. The entity's timeline does
//!    not change yet. A switch that snaps saves nothing.
//! 2. `apply_saved_switches` runs in `PreUpdate` of frame F+1, after
//!    replication receive and before the rollback check. It swaps the
//!    [`Predicted`] / [`Interpolated`] markers. For a switch to the prediction
//!    timeline it also asks for a forced rollback, so that in this same frame
//!    the world rewinds to K and replays under the new marker, leaving fresh
//!    simulated values in place.
//! 3. The shared creation system in [`crate::correction`] records the blend's
//!    error in `PostUpdate` of frame F+1, after frame interpolation: the jump
//!    from the value saved in step 1 to whatever the destination timeline has
//!    now. It runs every frame, and keeps the window's error up to date for as
//!    long as the window is live.
//!
//! `expire_switch_blends` ends the window and clears what it kept.
//!
//! # Why the value is saved before the switch
//!
//! The value to blend from has to be the one the player is looking at, and only
//! the end of the frame that renders it knows that value: frame interpolation
//! writes it, and the correction apply for the previous window writes on top of
//! it. Saving in step 1 and switching in step 2 is what makes the saved value
//! match the screen. A switch to the prediction timeline needs one more step
//! than that, because the forced rollback can only be requested before the
//! rollback check of a frame, which is in `PreUpdate`: switching on frame F
//! itself would either miss the rollback check or run the rollback before the
//! frame the request was sent in had rendered.
//!
//! # How a blend relates to `VisualCorrection`
//!
//! A blend *is* a correction. There is one error per corrected component, one
//! record of one jump:
//!
//! ```text
//! rendered = destination + error    (the apply adds the error every frame)
//! error    = previous - destination (recorded once, when the window opens)
//! ```
//!
//! and one decay that gives it up. The switch does not own a second error and
//! does not stack its error on top of a correction: it records the same kind of
//! measurement the rollback machinery records, in the same
//! [`PreviousVisual`](crate::correction::PreviousVisual) slot, and its window
//! decides how that error is decayed.
//!
//! Only the *schedule* is a blend's own. While a window is live its ease curve
//! scales the error each frame, so `with_transition_secs` and `with_ease` are
//! what shape the visible transition. That curve is floored by the entity's
//! [`CorrectionPolicy`](crate::correction::CorrectionPolicy): a curve's tail is
//! steep, so an error arriving late in the window would otherwise be dumped into
//! a single frame. A converged error is dropped by the ordinary small-error
//! check, which is what ends a blend early.
//!
//! Because every measurement is taken against what is on screen, which already
//! contains the previous offset, a new jump *replaces* the error instead of
//! adding to it. That settles the cases:
//!
//! * **Switching while a correction is decaying.** The saved value is the
//!   rendered value, which already carries the correction, so the switch takes
//!   the correction over: the error becomes `saved - new destination`, and the
//!   window's tuning replaces whatever the old correction had, including any
//!   time it had left.
//! * **A rollback during a blend.** The rollback captures the value on screen as
//!   usual, and the shared creation system measures the jump from it in
//!   `PostUpdate` — the same pass it runs for any jump. The window keeps
//!   governing the decay, so the blend is not cut short by the rollback, the
//!   render does not jump by the rollback's delta, and the rollback is not dumped
//!   by the curve's steep tail.
//! * **The window ending.** The expiry pass clears the state the window kept,
//!   and any error left is decayed by the correction policy.
//!
//! The forced rollback that a switch to the prediction timeline asks for is the
//! same machinery, and the two captures it involves agree: it takes the value on
//! screen in `PreUpdate` of the frame the markers swap, and nothing writes the
//! live value between the switch's save and that point, so it does not matter
//! which one a reader sees.
//!
//! Nothing here adds a decay system of its own and nothing touches physics:
//! simulation components stay owned by user systems. Switching is client-local,
//! only the spatial/visual error is smoothed, and an entity with a window open
//! ignores new requests until it ends.
//!
//! [`Predicted`]: lightyear_core::prediction::Predicted
//! [`Interpolated`]: lightyear_core::interpolation::Interpolated

use crate::correction::{CorrectionEase, CorrectionWorld};
use crate::manager::StateRollbackMetadata;
use crate::plugin::PredictionSystems;
use crate::registry::PredictionRegistry;
use crate::rollback::RollbackSystems;
use bevy_app::prelude::*;
use bevy_ecs::message::MessageCursor;
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
            default_transition_secs: 1.0,
            default_ease: CorrectionEase::EaseOutCubic,
        }
    }
}

/// Request to move an entity between the prediction and interpolation timelines.
///
/// Send the message — it is one-shot, so there is no component to insert or
/// remove, and callers never touch [`Predicted`] / [`Interpolated`] directly:
/// ```rust,ignore
/// switch_events.write(TimelineSwitch::to_predicted(entity));
/// // ...or blend over an explicit window instead of the default:
/// switch_events.write(
///     TimelineSwitch::to_predicted(entity).with_transition_secs(1.0),
/// );
/// ```
///
/// A request is handled in three passes; the module documentation has the full
/// order. In short: the value on screen is saved at the end of the frame the
/// request is seen, the markers are swapped at the start of the next frame (with
/// a forced rollback when moving to the prediction timeline), and the blend
/// error is written at the end of that frame and every frame until the window
/// ends.
///
/// The blend length is the per-request override when present, otherwise the
/// [`TimelineSwitchSettings`] default: `<= 0` (or non-finite) snaps instantly,
/// otherwise the jump is smoothed over the window and new requests for the
/// entity are ignored while it runs.
///
/// The blend reuses the shared
/// [`VisualCorrection`](crate::correction::VisualCorrection) machinery: the
/// switch records the jump between the value on screen and the destination, and
/// the window's ease curve scales how fast that error is given up.
#[derive(Message, Debug, Clone, Copy, PartialEq)]
pub struct TimelineSwitch {
    entity: Entity,
    direction: SwitchDirection,
    transition_secs: Option<f32>,
    ease: Option<CorrectionEase>,
}

impl TimelineSwitch {
    /// Switch `entity` from interpolated to predicted, blending over the
    /// [`TimelineSwitchSettings`] default unless overridden with
    /// [`with_transition_secs`](Self::with_transition_secs).
    ///
    /// The value the entity is rendering is saved at the end of the frame the
    /// request is seen, and `PreUpdate` of the next frame asks for a forced
    /// rollback at K (the latest rollback-scanned tick) while it swaps the
    /// markers. So the world rewinds and replays under the new [`Predicted`]
    /// marker in that frame, and the blend smooths the jump between the saved
    /// value and the replayed one. Before the first sync there is no K to
    /// rewind to, so the switch only swaps the markers and the next natural
    /// rollback corrects. No exclusion from rollback is needed afterwards: the
    /// switched entity participates like any other predicted entity.
    pub fn to_predicted(entity: Entity) -> Self {
        Self {
            entity,
            direction: SwitchDirection::ToPredicted,
            transition_secs: None,
            ease: None,
        }
    }

    /// Switch `entity` from predicted to interpolated, blending over the
    /// [`TimelineSwitchSettings`] default unless overridden with
    /// [`with_transition_secs`](Self::with_transition_secs).
    ///
    /// The value the entity is rendering is saved at the end of the frame the
    /// request is seen, and `PreUpdate` of the next frame swaps the markers.
    /// Delayed interpolation writes its first value later in that frame, and
    /// the blend then smooths the jump between the saved value and it. (The
    /// reverse direction, [`Self::to_predicted`], needs the forced rollback: it
    /// has to produce fresh simulation from confirmed data, while this direction
    /// only presents the delayed value.)
    pub fn to_interpolated(entity: Entity) -> Self {
        Self {
            entity,
            direction: SwitchDirection::ToInterpolated,
            transition_secs: None,
            ease: None,
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

    /// Which entity is moving.
    pub fn entity(&self) -> Entity {
        self.entity
    }

    /// Where the entity is moving.
    pub fn direction(&self) -> SwitchDirection {
        self.direction
    }

    /// Per-request blend-length override, if any. `None` resolves against
    /// [`TimelineSwitchSettings`] when the switch handlers apply the request.
    pub fn transition_override(&self) -> Option<f32> {
        self.transition_secs
    }

    /// Per-request ease-curve override, if any. `None` resolves against
    /// [`TimelineSwitchSettings`] when the switch handlers apply the request.
    pub fn ease_override(&self) -> Option<CorrectionEase> {
        self.ease
    }

    /// Fills in the blend length and ease curve from `settings`.
    ///
    /// Resolved when the request is saved rather than when it is sent, so games
    /// can retune mid-session. It happens a frame before the request is
    /// applied, and the resolved values are carried on the saved request, so
    /// the save and the apply agree on whether there is a blend at all.
    fn resolve_blend(&mut self, settings: &TimelineSwitchSettings) {
        self.transition_secs = Some(
            self.transition_secs
                .unwrap_or(settings.default_transition_secs),
        );
        self.ease = Some(self.ease.unwrap_or(settings.default_ease));
    }

    /// Whether the request blends instead of snapping. `<= 0` (or non-finite)
    /// snaps.
    fn blends(&self) -> bool {
        self.transition_secs
            .is_some_and(|secs| secs.is_finite() && secs > 0.0)
    }
}

/// Cursor over the [`TimelineSwitch`] messages.
#[derive(Resource, Default)]
pub(crate) struct SwitchRequestCursor(MessageCursor<TimelineSwitch>);

/// A request whose value on screen has been saved, waiting for its markers to
/// be swapped on the next frame.
///
/// Set by `save_previous_visuals` in `PostUpdate` of the frame the request is
/// seen, and turned into a [`SwitchBlend`] window by `apply_saved_switches` in
/// `PreUpdate` of the next frame. A request for an entity that already has this
/// component replaces it, so a policy that re-sends every frame is carried once
/// and the last request sent within a frame wins.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub(crate) struct PendingSwitch {
    direction: SwitchDirection,
    /// Blend length in seconds, already resolved against
    /// [`TimelineSwitchSettings`]. `<= 0` (or non-finite) snaps.
    total_secs: f32,
    /// Ease curve shaping the blend, already resolved.
    ease: CorrectionEase,
}

impl PendingSwitch {
    /// Whether this switch blends instead of snapping.
    fn blends(&self) -> bool {
        self.total_secs.is_finite() && self.total_secs > 0.0
    }
}

/// Marks an entity with an in-flight timeline-switch blend, and carries the
/// blend's shared schedule.
///
/// One window per switch, not one per corrected type: every corrected component
/// gets its own error against this same schedule, so the clock is read off the
/// marker rather than accumulated per type and can never drift between types.
/// The marker also separates entities that are blending for the archetype scans
/// and backs the query filters (`Without` / `Has`). While present, new
/// [`TimelineSwitch`] requests for the entity are ignored.
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
/// frame the request was sent from. What each corrected type saves depends on
/// where the entity is going — see the save handler in [`crate::correction`] —
/// and [`PendingSwitch`] carries the request to the next frame.
///
/// Requests for entities that are despawned, already blending, or not on the
/// timeline they want to leave are dropped.
pub(crate) fn save_previous_visuals(
    correction_world: CorrectionWorld,
    registry: Res<PredictionRegistry>,
    settings: Res<TimelineSwitchSettings>,
    mut cursor: ResMut<SwitchRequestCursor>,
    messages: Res<Messages<TimelineSwitch>>,
    mut commands: Commands,
) {
    let world = correction_world.world();
    for request in cursor.0.read(&messages) {
        let entity = request.entity;
        let Ok(entity_cell) = world.get_entity(entity) else {
            trace!(?entity, "dropping timeline switch for despawned entity");
            continue;
        };
        let is_predicted = entity_cell.contains::<Predicted>();
        let is_interpolated = entity_cell.contains::<Interpolated>();
        // Saving while a blend is running would fight the window that is still
        // converging, so the request waits for it to end. A policy that
        // re-sends every frame produces this routinely, so it is not a user
        // error; it is also what makes a repeated request harmless.
        if entity_cell.contains::<SwitchBlend>() {
            trace!(?entity, "dropping switch while a blend is active");
            continue;
        }
        match request.direction {
            SwitchDirection::ToPredicted if !is_interpolated => {
                warn!(
                    ?entity,
                    ?is_predicted,
                    ?is_interpolated,
                    "dropping invalid switch to predicted: entity must be interpolated"
                );
                continue;
            }
            SwitchDirection::ToInterpolated if !is_predicted => {
                warn!(
                    ?entity,
                    ?is_predicted,
                    ?is_interpolated,
                    "dropping invalid switch to interpolated: entity must be predicted"
                );
                continue;
            }
            _ => {}
        }
        // Blend length and curve are resolved here rather than when the request
        // is sent, so games can retune mid-session. The resolved values ride on
        // the pending switch so the save and the apply agree on whether there is
        // a blend at all.
        let total_secs = request
            .transition_secs
            .unwrap_or(settings.default_transition_secs);
        let ease = request.ease.unwrap_or(settings.default_ease);
        let pending = PendingSwitch {
            direction: request.direction,
            total_secs,
            ease,
        };
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
        saves.insert(entity, pending);
        saves.apply(&mut commands);
        trace!(
            ?entity,
            direction = ?request.direction,
            total_secs,
            "saved switch visual"
        );
    }
}

/// Swaps the timeline markers of every switch whose value was saved last frame.
///
/// Runs in `PreUpdate` after replication receive and before the rollback check,
/// which is what lets a switch to the prediction timeline ask for a forced
/// rollback and have the world rewound and replayed under the new markers in
/// this same frame. The shared creation system in [`crate::correction`] records
/// the blend's error against the replayed value later this frame.
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
///
/// Expiry cannot live in the blend system alone: that only runs for entities
/// with a live window, but the marker must also lift when an external removal
/// leaves a window with nothing to blend. Without this, an entity could never
/// switch again. Runs after the correction apply, so a live window still
/// converges exactly on its expiry pass first.
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
    app.add_message::<TimelineSwitch>();
    app.init_resource::<TimelineSwitchSettings>();
    app.init_resource::<SwitchRequestCursor>();
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
        // Same message plus per-handler cursors the plugin installs via
        // `add_timeline_switch_systems`. (Requests are one-shot messages, so
        // there is no request component to insert or remove.)
        app.add_message::<TimelineSwitch>();
        app.init_resource::<SwitchRequestCursor>();
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
    /// requests sent since the last one.
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

    /// Sends one request and runs the two passes, the way the schedule does
    /// over two frames: the value is saved at the end of the frame the request
    /// is seen, and the markers swap at the start of the next one.
    fn send_switch(app: &mut App, request: TimelineSwitch) {
        app.world_mut().write_message(request);
        run_save(app);
        run_apply(app);
    }

    /// Shared blend window stamped on `entity` by the switch handler.
    fn blend_marker(world: &World, entity: Entity) -> SwitchBlend {
        *world.get::<SwitchBlend>(entity).expect("blend marker")
    }

    #[test]
    fn prediction_plugin_registers_switch_pipeline() {
        let mut app = App::new();
        app.add_plugins((bevy_time::TimePlugin, crate::plugin::PredictionPlugin));
        app.init_resource::<lightyear_core::prelude::LocalTimeline>();
        app.init_resource::<lightyear_sync::prelude::LocalTimelineSync>();
        // Schedule build validates the switch systems' ordering constraints.
        app.update();
        assert!(app.world().contains_resource::<TimelineSwitchSettings>());
    }

    #[test]
    fn switch_to_predicted_swaps_markers_and_saves_visual() {
        let mut app = switch_app();
        let entity = app.world_mut().spawn((TestPos(10.0), Interpolated)).id();
        app.world_mut().flush();

        let request = TimelineSwitch::to_predicted(entity).with_transition_secs(0.5);
        assert_eq!(request.direction(), SwitchDirection::ToPredicted);
        assert_eq!(request.transition_override(), Some(0.5));
        send_switch(&mut app, request);

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
            TimelineSwitch::to_interpolated(entity).with_transition_secs(0.25),
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
            TimelineSwitch::to_interpolated(entity).with_transition_secs(0.5),
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

    /// The default ease has to release a useful amount of the gap on the first
    /// frame. A curve with a flat start holds almost all of it — under 1% of the
    /// gap for a frame of a one second window — which is a stop that then lurches,
    /// and that is what the default used to be.
    #[test]
    fn default_ease_advances_the_render_immediately() {
        let frame = 1.0 / 60.0;
        let window = 1.0;
        let default = TimelineSwitchSettings::default().default_ease;
        let first_frame_keep = default.keep(0.0, frame, window);
        assert!(
            first_frame_keep <= 0.96,
            "the default {default:?} holds {first_frame_keep} of the gap on the first frame"
        );
        // A flat-starting curve is kept for callers who want it, but it is not
        // the default for exactly this reason.
        let flat = CorrectionEase::Smoothstep.keep(0.0, frame, window);
        assert!(
            flat > 0.999,
            "smoothstep should hold nearly the whole gap, got {flat}"
        );
    }

    /// A window may run the unbounded curve: then the window is what bounds it.
    /// The ratio is constant per frame instead of following a normalised curve,
    /// and the window still ends on its own length.
    #[test]
    fn window_can_run_the_exponential_ease() {
        use crate::correction::update_visual_correction;
        use crate::manager::PredictionManager;
        use bevy_ecs::system::RunSystemOnce;

        let mut app = switch_app();
        app.insert_resource(PredictionManager::default());
        let exponential = CorrectionEase::Exponential {
            decay_ratio: 0.5,
            decay_period_secs: 0.2,
        };
        let entity = app
            .world_mut()
            .spawn((
                TestPos(10.0),
                Predicted,
                SwitchBlend::new(0.0, 0.5, exponential),
                VisualCorrection::new(TestPos(-1.0), 0.0),
            ))
            .id();
        app.world_mut().flush();

        // Two frames of the same length give up the same fraction: the
        // exponential has no curve to advance along.
        let mut ratios = alloc::vec::Vec::new();
        for _ in 0..2 {
            let before = app
                .world()
                .get::<VisualCorrection<TestPos>>(entity)
                .map(|c| c.error.0)
                .unwrap();
            app.world_mut()
                .resource_mut::<Time<Virtual>>()
                .advance_by(Duration::from_millis(16));
            app.world_mut()
                .run_system_once(update_visual_correction::<TestPos, TestPos>)
                .unwrap();
            app.world_mut().flush();
            let after = app
                .world()
                .get::<VisualCorrection<TestPos>>(entity)
                .map(|c| c.error.0)
                .unwrap();
            ratios.push(after / before);
        }
        assert!(
            (ratios[0] - ratios[1]).abs() < 1e-5,
            "an exponential gives up the same fraction each frame, got {ratios:?}"
        );
        // The policy's rate is a floor, and here it is the same curve, so the
        // observed ratio is that of one 16 ms step.
        assert!(
            (ratios[0] - exponential.remaining(0.016, 1.0)).abs() < 1e-5,
            "got {}",
            ratios[0]
        );
    }

    /// A blend's error decays on its window's curve, not on the entity's
    /// correction policy: the length and curve the caller asked for are what the
    /// transition looks like.
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
        app.world_mut().flush();

        app.world_mut()
            .resource_mut::<Time<Virtual>>()
            .advance_by(Duration::from_millis(50));
        app.world_mut()
            .run_system_once(update_visual_correction::<TestPos, TestPos>)
            .unwrap();
        app.world_mut().flush();

        let blend_error = app
            .world()
            .get::<VisualCorrection<TestPos>>(blended)
            .map(|c| c.error.0)
            .unwrap();
        let policy_error = app
            .world()
            .get::<VisualCorrection<TestPos>>(policy)
            .map(|c| c.error.0)
            .unwrap();
        assert!(
            blend_error < policy_error,
            "the window's curve should hold the error longer than the policy, \
             got blend {blend_error} and policy {policy_error}"
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
            .write_message(TimelineSwitch::to_interpolated(entity).with_transition_secs(0.5));
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
            .write_message(TimelineSwitch::to_predicted(entity).with_transition_secs(0.5));
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
            .write_message(TimelineSwitch::to_predicted(entity).with_transition_secs(0.5));
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
            .write_message(TimelineSwitch::to_predicted(entity).with_transition_secs(0.5));
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

    /// A rollback landing during a window replaces the error and is then given
    /// up on the window's curve, floored by the correction policy. The hold the
    /// rollback recorded has to stay: neither dropped by the switch nor dumped by
    /// the curve's steep tail.
    #[test]
    fn rollback_during_a_blend_stays_smoothed() {
        use crate::correction::update_visual_correction;
        use crate::manager::PredictionManager;
        use bevy_ecs::system::RunSystemOnce;

        let mut app = switch_app();
        app.insert_resource(PredictionManager::default());
        // 90% through a half second window. The window has already taken its
        // jump (no saved value left), and a rollback has just recorded the move
        // from 10 to 20 as the error.
        let entity = app
            .world_mut()
            .spawn((
                TestPos(20.0),
                Predicted,
                SwitchBlend::new(0.0, 0.5, CorrectionEase::Smoothstep),
                VisualCorrection::new(TestPos(10.0 - 20.0), 0.0),
            ))
            .id();
        app.world_mut().flush();
        // Move the clock to 90% of the window without turning this frame into a
        // 450 ms frame, which would decay the error by the whole elapsed time.
        app.world_mut()
            .resource_mut::<Time<Virtual>>()
            .advance_by(Duration::from_millis(434));
        app.world_mut()
            .resource_mut::<Time<Virtual>>()
            .advance_by(Duration::from_millis(16));

        run_blend(&mut app);
        app.world_mut()
            .run_system_once(update_visual_correction::<TestPos, TestPos>)
            .unwrap();
        app.world_mut().flush();

        let rendered = app.world().get::<TestPos>(entity).unwrap().0;
        assert!(
            (rendered - 10.0).abs() < 1.5,
            "the rollback's hold must stay, got {rendered} (jumped toward 20)"
        );
        // The curve alone would have dumped most of it this late in the window;
        // the correction policy's rate is the floor.
        let error = app
            .world()
            .get::<VisualCorrection<TestPos>>(entity)
            .map(|c| c.error.0)
            .unwrap();
        assert!(
            (error + 10.0).abs() < 1.5,
            "the error should be held, got {error}"
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
            .write_message(TimelineSwitch::to_interpolated(entity).with_transition_secs(0.5));
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
            TimelineSwitch::to_interpolated(entity).with_transition_secs(0.5),
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

        send_switch(&mut app, TimelineSwitch::to_interpolated(entity));

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

    /// The window ends on its own: the marker and everything it kept go, and by
    /// then the blend has given up the whole error, so the entity is exactly on
    /// its destination value. A correction from a rollback that landed during
    /// the window does not survive either — the window owned the error for its
    /// whole length.
    #[test]
    fn switch_to_predicted_drops_stale_frame_histories() {
        let mut app = switch_app();
        let entity = app.world_mut().spawn((TestPos(0.6), Interpolated)).id();
        // History written before the switch to interpolation: while
        // `FrameInterpolate` was absent no frame history was recorded, so these
        // values are older than the live value by that whole period.
        app.world_mut()
            .entity_mut(entity)
            .insert(FrameInterpolationHistory::<TestPos> {
                previous_value: Some(TestPos(47.0)),
                current_value: Some(TestPos(47.7)),
            });
        app.world_mut().flush();

        send_switch(&mut app, TimelineSwitch::to_predicted(entity));

        let world = app.world();
        assert!(world.get::<Predicted>(entity).is_some());
        assert!(world.get::<FrameInterpolate>(entity).is_some());
        // The old history must be gone: re-adding the marker without dropping
        // it would make frame restore put +47.7 back onto live simulation, and
        // the switch blend would read it as the value on screen. The next
        // history update starts again from the live value instead.
        assert!(
            world
                .get::<FrameInterpolationHistory<TestPos>>(entity)
                .is_none()
        );
        assert_eq!(world.get::<TestPos>(entity), Some(&TestPos(0.6)));
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
        send_switch(&mut app, TimelineSwitch::to_predicted(entity));
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
        send_switch(&mut app, TimelineSwitch::to_interpolated(entity));
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
                TimelineSwitch::to_predicted(entity).with_transition_secs(duration),
            );

            let world = app.world();
            assert!(world.get::<Predicted>(entity).is_some());
            assert!(world.get::<Interpolated>(entity).is_none());
            assert!(world.get::<PreviousVisual<TestPos>>(entity).is_none());
            assert!(world.get::<SwitchBlend>(entity).is_none());
        }
    }

    #[test]
    fn bare_request_blends_over_resource_default() {
        let mut app = switch_app();
        let entity = app.world_mut().spawn((TestPos(1.0), Interpolated)).id();
        app.world_mut().flush();

        let request = TimelineSwitch::to_predicted(entity);
        assert_eq!(request.transition_override(), None);
        send_switch(&mut app, request);

        assert!(app.world().get::<SwitchBlend>(entity).is_some());
        assert_eq!(
            blend_marker(app.world(), entity).total_secs(),
            TimelineSwitchSettings::default().default_transition_secs
        );
    }

    #[test]
    fn explicit_override_wins_over_customized_default() {
        let mut app = switch_app();
        app.world_mut().insert_resource(TimelineSwitchSettings {
            default_transition_secs: 2.0,
            default_ease: CorrectionEase::Linear,
        });
        let bare = app.world_mut().spawn((TestPos(1.0), Interpolated)).id();
        let overridden = app.world_mut().spawn((TestPos(2.0), Interpolated)).id();
        app.world_mut().flush();

        send_switch(&mut app, TimelineSwitch::to_predicted(bare));
        send_switch(
            &mut app,
            TimelineSwitch::to_predicted(overridden).with_transition_secs(0.5),
        );

        let world = app.world();
        assert_eq!(blend_marker(world, bare).total_secs(), 2.0);
        assert_eq!(blend_marker(world, overridden).total_secs(), 0.5);
    }

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
        // Spot values pin the curves (smoothstep and cubic ease-out).
        assert!((CorrectionEase::Smoothstep.remaining(0.5, 1.0) - 0.5).abs() < 1e-6);
        assert!((CorrectionEase::EaseOutCubic.remaining(0.25, 1.0) - 0.421875).abs() < 1e-6);
        assert!((CorrectionEase::EaseInOutCubic.remaining(0.5, 1.0) - 0.5).abs() < 1e-6);
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
    fn switch_resolves_ease_default_and_override() {
        let mut app = switch_app();
        app.world_mut().insert_resource(TimelineSwitchSettings {
            default_transition_secs: 0.5,
            default_ease: CorrectionEase::Smoothstep,
        });
        let bare = app.world_mut().spawn((TestPos(1.0), Interpolated)).id();
        let overridden = app.world_mut().spawn((TestPos(2.0), Interpolated)).id();
        app.world_mut().flush();

        send_switch(&mut app, TimelineSwitch::to_predicted(bare));
        send_switch(
            &mut app,
            TimelineSwitch::to_predicted(overridden).with_ease(CorrectionEase::EaseOutCubic),
        );

        let world = app.world();
        assert_eq!(blend_marker(world, bare).ease(), CorrectionEase::Smoothstep);
        assert_eq!(
            blend_marker(world, overridden).ease(),
            CorrectionEase::EaseOutCubic
        );
    }

    #[test]
    fn user_inserted_new_marker_only_needs_removal() {
        let mut app = switch_app();
        // User systems already inserted Predicted; the switch only removes Interpolated.
        let entity = app
            .world_mut()
            .spawn((TestPos(7.0), Predicted, Interpolated))
            .id();
        app.world_mut().flush();

        send_switch(
            &mut app,
            TimelineSwitch::to_predicted(entity).with_transition_secs(0.5),
        );

        let world = app.world();
        assert!(world.get::<Predicted>(entity).is_some());
        assert!(world.get::<Interpolated>(entity).is_none());
        assert!(world.get::<SwitchBlend>(entity).is_some());
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
            TimelineSwitch::to_predicted(predicted).with_transition_secs(0.5),
        );
        send_switch(
            &mut app,
            TimelineSwitch::to_interpolated(interpolated).with_transition_secs(0.5),
        );
        send_switch(
            &mut app,
            TimelineSwitch::to_predicted(plain).with_transition_secs(0.5),
        );

        let world = app.world();
        assert!(world.get::<Predicted>(predicted).is_some());
        assert!(world.get::<SwitchBlend>(predicted).is_none());
        assert!(world.get::<Interpolated>(interpolated).is_some());
        assert!(world.get::<SwitchBlend>(interpolated).is_none());
        assert!(world.get::<SwitchBlend>(plain).is_none());

        // A second request while blending is ignored.
        send_switch(
            &mut app,
            TimelineSwitch::to_predicted(interpolated).with_transition_secs(0.5),
        );
        {
            let world = app.world();
            assert!(world.get::<Predicted>(interpolated).is_some());
            assert!(world.get::<SwitchBlend>(interpolated).is_some());
        }
        send_switch(
            &mut app,
            TimelineSwitch::to_interpolated(interpolated).with_transition_secs(0.5),
        );
        let world = app.world();
        assert!(world.get::<Predicted>(interpolated).is_some());
        assert!(world.get::<Interpolated>(interpolated).is_none());
        assert!(world.get::<SwitchBlend>(interpolated).is_some());
    }

    #[test]
    fn despawned_entity_takes_pending_request_with_it() {
        // Requests are one-shot messages, so one can outlive its entity: the
        // handler must simply skip it, no tombstoning needed.
        let mut app = switch_app();
        let entity = app.world_mut().spawn((TestPos(10.0), Interpolated)).id();
        app.world_mut().flush();

        app.world_mut()
            .write_message(TimelineSwitch::to_predicted(entity).with_transition_secs(0.5));
        app.world_mut().despawn(entity);
        run_save(&mut app);
        run_apply(&mut app);
    }

    #[test]
    fn expired_switch_window_clears_state_of_missing_component() {
        use bevy_ecs::system::RunSystemOnce;

        let mut app = switch_app();
        let entity = app.world_mut().spawn((TestPos(10.0), Predicted)).id();
        app.world_mut().flush();
        send_switch(
            &mut app,
            TimelineSwitch::to_interpolated(entity).with_transition_secs(0.5),
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

    /// The forced rollback's EndRollback correction creation runs between the
    /// drain's capture and the switch creation. It must not consume a capture
    /// that belongs to a pending switch blend: the switch creation owns that
    /// blend.
    #[test]
    fn switch_requests_forced_rollback_at_last_processed_tick() {
        // No readiness check: even a young entity switches immediately, and
        // the same-frame rollback check rewinds the world to the scanned
        // frontier.
        let mut app = switch_app();
        let k = Tick(90);
        app.init_resource::<StateRollbackMetadata>();
        app.world_mut()
            .resource_mut::<StateRollbackMetadata>()
            .set_last_processed_confirmed_tick(k);
        let entity = app.world_mut().spawn((TestPos(10.0), Interpolated)).id();
        app.world_mut().flush();

        send_switch(
            &mut app,
            TimelineSwitch::to_predicted(entity).with_transition_secs(0.5),
        );

        let world = app.world();
        assert!(world.get::<Predicted>(entity).is_some());
        assert!(world.get::<Interpolated>(entity).is_none());
        assert!(world.get::<SwitchBlend>(entity).is_some());
        assert_eq!(
            world
                .resource::<StateRollbackMetadata>()
                .forced_rollback_tick(),
            Some(k)
        );
    }

    #[test]
    fn switch_to_interpolated_requests_no_rollback() {
        // A switch to interpolation needs no forced rollback: delayed
        // interpolation writes the delayed value once the markers are swapped,
        // and the blend smooths the jump.
        let mut app = switch_app();
        app.init_resource::<StateRollbackMetadata>();
        app.world_mut()
            .resource_mut::<StateRollbackMetadata>()
            .set_last_processed_confirmed_tick(Tick(90));
        let entity = app.world_mut().spawn((TestPos(10.0), Predicted)).id();
        app.world_mut().flush();

        send_switch(
            &mut app,
            TimelineSwitch::to_interpolated(entity).with_transition_secs(0.5),
        );

        let world = app.world();
        assert!(world.get::<Predicted>(entity).is_none());
        assert!(world.get::<Interpolated>(entity).is_some());
        assert!(world.get::<SwitchBlend>(entity).is_some());
        assert_eq!(
            world
                .resource::<StateRollbackMetadata>()
                .forced_rollback_tick(),
            None
        );
    }

    #[test]
    fn switch_without_scanned_tick_requests_no_rollback() {
        // Before the first sync no rollback check has run, so there is no K to
        // rewind to: the switch still applies, just without requesting a
        // rollback.
        let mut app = switch_app();
        app.init_resource::<StateRollbackMetadata>();
        let entity = app.world_mut().spawn((TestPos(10.0), Interpolated)).id();
        app.world_mut().flush();

        send_switch(
            &mut app,
            TimelineSwitch::to_predicted(entity).with_transition_secs(0.5),
        );

        let world = app.world();
        assert!(world.get::<Predicted>(entity).is_some());
        assert_eq!(
            world
                .resource::<StateRollbackMetadata>()
                .forced_rollback_tick(),
            None
        );
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
            .write_message(TimelineSwitch::to_predicted(entity).with_transition_secs(0.5));
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

    #[test]
    fn zero_error_blend_correction_survives_applies_while_marker_live() {
        use crate::correction::update_visual_correction;
        use crate::manager::PredictionManager;
        use bevy_ecs::system::RunSystemOnce;

        // A static entity whose destination value agrees with the one on screen:
        // the seed records an offset of exactly zero, which is dropped like any
        // converged offset. Nothing re-derives it, so the window does not need to
        // keep it alive: a later rollback records a fresh correction.
        let mut app = switch_app();
        app.insert_resource(PredictionManager::default());
        let entity = app.world_mut().spawn((TestPos(5.0), Predicted)).id();
        app.world_mut().flush();

        send_switch(
            &mut app,
            TimelineSwitch::to_interpolated(entity).with_transition_secs(1.0),
        );
        // One frame later: the entry is promoted, and the destination
        // (delayed) value already landed. Here it equals the saved value, so
        // the measured error is exactly zero.
        run_blend(&mut app);
        run_blend(&mut app);
        assert_eq!(
            app.world()
                .get::<VisualCorrection<TestPos>>(entity)
                .map(|c| c.error.clone()),
            Some(TestPos(0.0))
        );

        for _ in 0..3 {
            app.world_mut()
                .resource_mut::<Time<Virtual>>()
                .advance_by(Duration::from_millis(16));
            app.world_mut()
                .run_system_once(update_visual_correction::<TestPos, TestPos>)
                .unwrap();
            assert!(
                app.world()
                    .get::<VisualCorrection<TestPos>>(entity)
                    .is_none(),
                "a converged offset is dropped whether or not a window is live"
            );
            assert!(
                app.world().get::<SwitchBlend>(entity).is_some(),
                "the window itself lives out its length"
            );
        }
    }

    /// A request only starts from the timeline the entity is on when the
    /// request is seen. Two requests that contradict each other in one frame
    /// cannot both be carried, so the one whose source timeline matches the
    /// entity is the one that survives; the other is dropped as invalid.
    #[test]
    fn contradictory_requests_in_one_frame_keep_the_matching_one() {
        for (first, second, expect_predicted) in [
            // Up first, then down: the down request wants to leave the
            // prediction timeline, which the entity only reaches when the up
            // request is applied, so it is dropped and the up one is applied.
            (
                SwitchDirection::ToPredicted,
                SwitchDirection::ToInterpolated,
                true,
            ),
            // Down first, then up: the down request wants to leave the
            // prediction timeline, which this entity is not on, so it is
            // dropped and the up one is applied.
            (
                SwitchDirection::ToInterpolated,
                SwitchDirection::ToPredicted,
                true,
            ),
        ] {
            let mut app = switch_app();
            let entity = app.world_mut().spawn((TestPos(1.0), Interpolated)).id();
            app.world_mut().flush();

            let send = |direction| match direction {
                SwitchDirection::ToPredicted => TimelineSwitch::to_predicted(entity),
                SwitchDirection::ToInterpolated => TimelineSwitch::to_interpolated(entity),
            };
            app.world_mut().write_message(send(first));
            app.world_mut().write_message(send(second));
            run_save(&mut app);
            run_apply(&mut app);

            let world = app.world();
            assert_eq!(
                world.get::<Predicted>(entity).is_some(),
                expect_predicted,
                "{second:?} sent after {first:?} on an interpolated entity"
            );
        }
    }

    /// A policy that re-sends every frame must not apply the same switch twice:
    /// the entity is carried once per frame, and the request that sticks is the
    /// last one sent.
    #[test]
    fn repeated_requests_in_one_frame_carry_once() {
        let mut app = switch_app();
        let entity = app.world_mut().spawn((TestPos(1.0), Interpolated)).id();
        app.world_mut().flush();

        app.world_mut()
            .write_message(TimelineSwitch::to_predicted(entity).with_transition_secs(0.5));
        app.world_mut()
            .write_message(TimelineSwitch::to_predicted(entity).with_transition_secs(0.25));
        app.world_mut()
            .write_message(TimelineSwitch::to_predicted(entity).with_transition_secs(0.25));
        run_save(&mut app);
        run_apply(&mut app);

        let world = app.world();
        assert!(world.get::<Predicted>(entity).is_some());
        assert_eq!(blend_marker(world, entity).total_secs(), 0.25);
    }

    #[test]
    fn consumed_request_does_not_replay() {
        // One-shot semantics: running a handler with no new message changes
        // nothing, even right after a switch.
        let mut app = switch_app();
        let entity = app.world_mut().spawn((TestPos(1.0), Interpolated)).id();
        app.world_mut().flush();

        send_switch(&mut app, TimelineSwitch::to_predicted(entity));
        assert!(app.world().get::<Predicted>(entity).is_some());
        // No new message: both passes are noops.
        run_save(&mut app);
        run_apply(&mut app);
        assert!(app.world().get::<Predicted>(entity).is_some());
        assert!(app.world().get::<Interpolated>(entity).is_none());
    }
}
