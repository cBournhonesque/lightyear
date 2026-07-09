use avian3d::prelude::*;
use bevy::prelude::*;
use leafwing_input_manager::prelude::ActionState;
use lightyear::input::leafwing::prelude::LeafwingBuffer;
use lightyear::prediction::correction::VisualCorrection;
use lightyear::prelude::*;
use lightyear_frame_interpolation::FrameInterpolationHistory;

use crate::protocol::{BlockMarker, CharacterAction, CharacterMarker, ProjectileMarker};

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

fn emit_last_characters(
    timeline: Res<LocalTimeline>,
    players: Query<
        (
            Entity,
            &Position,
            &Transform,
            Option<&FrameInterpolationHistory<Position>>,
            Option<&VisualCorrection<Position>>,
        ),
        With<CharacterMarker>,
    >,
) {
    let tick = timeline.tick();

    for (entity, position, transform, interpolate, correction) in players.iter() {
        lightyear_debug_event!(
            DebugCategory::Component,
            DebugSamplePoint::Last,
            "Last",
            "character_last",
            tick = ?tick,
            entity = ?entity,
            position = ?position,
            transform = ?transform,
            interpolate = ?interpolate,
            correction = ?correction,
            "Player - Last"
        );
    }
}

#[cfg(feature = "client")]
pub(crate) mod client {
    use super::*;

    pub(crate) fn mark_debug_entities(
        mut commands: Commands,
        entities: Query<
            Entity,
            (
                With<Position>,
                Or<(
                    Added<CharacterMarker>,
                    Added<BlockMarker>,
                    Added<ProjectileMarker>,
                )>,
            ),
        >,
    ) {
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
                Or<(
                    Added<CharacterMarker>,
                    Added<BlockMarker>,
                    Added<ProjectileMarker>,
                )>,
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
