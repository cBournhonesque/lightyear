use avian2d::prelude::*;
use bevy::prelude::*;
use leafwing_input_manager::prelude::ActionState;
use lightyear::input::leafwing::prelude::LeafwingBuffer;
use lightyear::prediction::correction::VisualCorrection;
use lightyear::prelude::*;
use lightyear_frame_interpolation::FrameInterpolate;

use crate::protocol::{BallMarker, PlayerActions, PlayerId};

pub(crate) fn register_debug_systems(app: &mut App) {
    app.add_systems(
        FixedPostUpdate,
        emit_before_physics
            .after(PhysicsSystems::Prepare)
            .before(PhysicsSystems::StepSimulation),
    );
    app.add_systems(FixedLast, emit_fixed_last_players);
}

fn emit_before_physics(
    timeline: Res<LocalTimeline>,
    players: Query<
        (
            Entity,
            &Position,
            &Rotation,
            &LinearVelocity,
            &AngularVelocity,
            Option<&FrameInterpolate>,
            Option<&VisualCorrection<Position>>,
            Option<&ActionState<PlayerActions>>,
            Option<&LeafwingBuffer<PlayerActions>>,
            Option<&PlayerId>,
            Has<BallMarker>,
        ),
        Or<(With<PlayerId>, With<BallMarker>)>,
    >,
) {
    let tick = timeline.tick();
    for (
        entity,
        position,
        rotation,
        linear_velocity,
        angular_velocity,
        interpolate,
        correction,
        action_state,
        input_buffer,
        player_id,
        is_ball,
    ) in players.iter()
    {
        let pressed = action_state.map(|a| a.get_pressed());
        let last_buffer_tick = input_buffer.and_then(|b| b.get_last_with_tick().map(|(t, _)| t));
        lightyear_debug_event!(
            DebugCategory::Component,
            DebugSamplePoint::FixedUpdateBeforePhysics,
            "FixedPostUpdate",
            "player_before_physics",
            tick = ?tick,
            entity = ?entity,
            player_id = ?player_id,
            is_ball,
            position = ?position,
            rotation = ?rotation,
            linear_velocity = ?linear_velocity,
            angular_velocity = ?angular_velocity,
            interpolate = ?interpolate,
            correction = ?correction,
            pressed = ?pressed,
            last_buffer_tick = ?last_buffer_tick,
            "Player right before Physics::StepSimulation"
        );
    }
}

fn emit_fixed_last_players(
    timeline: Res<LocalTimeline>,
    players: Query<
        (
            Entity,
            &Position,
            &Rotation,
            &LinearVelocity,
            &AngularVelocity,
            Option<&FrameInterpolate>,
            Option<&VisualCorrection<Position>>,
            Option<&ActionState<PlayerActions>>,
            Option<&LeafwingBuffer<PlayerActions>>,
            Option<&PlayerId>,
            Has<BallMarker>,
        ),
        Or<(With<PlayerId>, With<BallMarker>)>,
    >,
) {
    let tick = timeline.tick();
    for (
        entity,
        position,
        rotation,
        linear_velocity,
        angular_velocity,
        interpolate,
        correction,
        action_state,
        input_buffer,
        player_id,
        is_ball,
    ) in players.iter()
    {
        let pressed = action_state.map(|a| a.get_pressed());
        let last_buffer_tick = input_buffer.and_then(|b| b.get_last_with_tick().map(|(t, _)| t));
        lightyear_debug_event!(
            DebugCategory::Component,
            DebugSamplePoint::FixedLast,
            "FixedLast",
            "player_fixed_last",
            tick = ?tick,
            entity = ?entity,
            player_id = ?player_id,
            is_ball,
            position = ?position,
            rotation = ?rotation,
            linear_velocity = ?linear_velocity,
            angular_velocity = ?angular_velocity,
            interpolate = ?interpolate,
            correction = ?correction,
            pressed = ?pressed,
            last_buffer_tick = ?last_buffer_tick,
            "Player in FixedLast"
        );
    }
}

#[cfg(feature = "client")]
pub(crate) mod client {
    use super::*;

    pub(crate) fn mark_debug_players(
        mut commands: Commands,
        query: Query<Entity, (Added<PlayerId>, With<Position>)>,
    ) {
        for entity in &query {
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

    pub(crate) fn mark_debug_players(
        mut commands: Commands,
        query: Query<Entity, (Added<PlayerId>, With<Position>)>,
    ) {
        for entity in &query {
            commands
                .entity(entity)
                .insert(LightyearDebug::component_at::<Position>([
                    DebugSamplePoint::Update,
                ]));
        }
    }
}
