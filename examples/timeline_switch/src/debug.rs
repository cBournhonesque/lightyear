use avian3d::prelude::*;
use bevy::prelude::*;
use leafwing_input_manager::prelude::ActionState;
use lightyear::input::leafwing::prelude::LeafwingBuffer;
use lightyear::prediction::correction::VisualCorrection;
use lightyear::prediction::switch::SwitchBlend;
use lightyear::prelude::*;
use lightyear_frame_interpolation::FrameInterpolationHistory;

use crate::protocol::{BlockMarker, CarriedBy, CharacterAction, CharacterMarker};

pub(crate) fn register_debug_systems(app: &mut App) {
    app.add_systems(FixedLast, emit_fixed_last_characters);
    app.add_systems(Last, emit_last_characters);
}

fn emit_fixed_last_characters(
    timeline: Res<LocalTimeline>,
    players: Query<
        (
            Entity,
            &Position,
            Option<&VisualCorrection<Position>>,
            Option<&ActionState<CharacterAction>>,
            Option<&LeafwingBuffer<CharacterAction>>,
        ),
        With<CharacterMarker>,
    >,
) {
    let tick = timeline.tick();

    for (entity, position, correction, action_state, input_buffer) in players.iter() {
        let pressed = action_state.map(|a| a.axis_pair(&CharacterAction::Move));
        let last_buffer_tick = input_buffer.and_then(|b| b.get_last_with_tick().map(|(t, _)| t));
        lightyear_debug_event!(
            DebugCategory::Component,
            DebugSamplePoint::FixedLast,
            "FixedLast",
            "character_fixed_last",
            tick = ?tick,
            entity = ?entity,
            position = ?position,
            correction = ?correction,
            pressed = ?pressed,
            last_buffer_tick = ?last_buffer_tick,
            "Player - FixedLast"
        );
    }
}

/// One row per character per rendered frame, with everything needed to explain
/// a visual discontinuity: what the render shows, what the simulation holds, the
/// error between them, and which timeline the entity is on.
///
/// Analyze with duckdb, e.g. the rendered jump per frame:
///
/// ```text
/// LIGHTYEAR_DEBUG_FILE=debug.jsonl RUST_LOG="lightyear_debug=trace" cargo run -p timeline_switch -- client
/// duckdb -c "select frame_id, entity, transform from read_json_auto('debug.jsonl')
///            where kind='character_last' order by frame_id"
/// ```
///
/// `blend` is the timeline-switch window and `correction` the remaining error, so
/// a switch shows up as the frame where a correction appears with a fresh
/// `start_secs`.
fn emit_last_characters(
    timeline: Res<LocalTimeline>,
    players: Query<
        (
            Entity,
            &Position,
            &Transform,
            Option<&LinearVelocity>,
            Option<&FrameInterpolationHistory<Position>>,
            Option<&VisualCorrection<Position>>,
            Has<Predicted>,
            Has<Interpolated>,
            Has<SwitchBlend>,
            Has<CarriedBy>,
        ),
        Or<(With<CharacterMarker>, With<BlockMarker>)>,
    >,
) {
    let tick = timeline.tick();

    for (
        entity,
        position,
        transform,
        velocity,
        interpolate,
        correction,
        predicted,
        interpolated,
        blend,
        carried,
    ) in players.iter()
    {
        lightyear_debug_event!(
            DebugCategory::Component,
            DebugSamplePoint::Last,
            "Last",
            "character_last",
            tick = ?tick,
            entity = ?entity,
            position = ?position,
            transform = ?transform,
            velocity = ?velocity,
            interpolate = ?interpolate,
            correction = ?correction,
            predicted = predicted,
            interpolated = interpolated,
            blend = blend,
            carried = carried,
            "Entity - Last"
        );
    }
}

#[cfg(feature = "client")]
pub(crate) mod client {
    use super::*;

    pub(crate) fn mark_debug_entities(
        mut commands: Commands,
        entities: Query<Entity, Or<(Added<CharacterMarker>, Added<BlockMarker>)>>,
    ) {
        // NOTE: no `With<Position>` filter on purpose. Replicated components
        // arrive in separate messages, so `Position` is usually added a few
        // frames after the marker; requiring both at once misses the `Added`
        // window and the entity is never sampled. The sampler skips entities
        // that are still missing `Position`.
        for entity in &entities {
            commands
                .entity(entity)
                .insert(LightyearDebug::component_at::<Position>([
                    DebugSamplePoint::Update,
                ]));
        }
    }
}

#[cfg(feature = "server")]
pub(crate) mod server {
    use super::*;

    pub(crate) fn mark_debug_entities(
        mut commands: Commands,
        entities: Query<
            Entity,
            (
                With<Position>,
                Or<(Added<CharacterMarker>, Added<BlockMarker>)>,
            ),
        >,
    ) {
        for entity in &entities {
            commands
                .entity(entity)
                .insert(LightyearDebug::component_at::<Position>([
                    DebugSamplePoint::FixedUpdate,
                ]));
        }
    }
}
