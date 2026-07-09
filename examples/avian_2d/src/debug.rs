use avian2d::prelude::*;
use bevy::prelude::*;
use leafwing_input_manager::input_map::InputMap;
use leafwing_input_manager::prelude::ActionState;
use lightyear::input::leafwing::prelude::LeafwingBuffer;
use lightyear::prediction::correction::VisualCorrection;
use lightyear::prelude::*;

use crate::protocol::{BallMarker, PlayerActions, PlayerId};

pub(crate) fn print_overstep(
    time: Res<Time<Fixed>>,
    timeline: Single<&InputTimeline, With<Client>>,
) {
    let input_overstep = timeline.overstep();
    let input_overstep_ms = input_overstep.to_f32() * (time.timestep().as_millis() as f32);
    let time_overstep = time.overstep();
    trace!(?input_overstep_ms, ?time_overstep, "overstep");
}

pub(crate) fn register_debug_systems(app: &mut App) {
    app.add_systems(
        FixedPostUpdate,
        emit_before_physics.before(PhysicsSystems::StepSimulation),
    );
    app.add_systems(FixedLast, emit_fixed_last_players);
    app.add_systems(
        RunFixedMainLoop,
        emit_fixed_loop_start.in_set(RunFixedMainLoopSystems::BeforeFixedMainLoop),
    );
}

fn emit_fixed_loop_start() {
    lightyear_debug_event!(
        DebugCategory::Timeline,
        DebugSamplePoint::RunFixedMainLoop,
        "RunFixedMainLoop",
        "fixed_loop_start",
        "Fixed Start"
    );
}

pub(crate) fn emit_fixed_pre_inputs(
    timeline: Res<LocalTimeline>,
    remote_client_inputs: Query<
        (
            Entity,
            &ActionState<PlayerActions>,
            &LeafwingBuffer<PlayerActions>,
        ),
        (Without<InputMap<PlayerActions>>, With<Predicted>),
    >,
) {
    let tick = timeline.tick();
    for (entity, action_state, buffer) in remote_client_inputs.iter() {
        let pressed = action_state.get_pressed();
        lightyear_debug_event!(
            DebugCategory::Input,
            DebugSamplePoint::FixedPreUpdate,
            "FixedPreUpdate",
            "remote_input_before_fixed_update",
            tick = ?tick,
            entity = ?entity,
            pressed = ?pressed,
            buffer = %buffer,
            "Remote client input before FixedUpdate"
        );
    }
}

fn emit_before_physics(
    timeline: Res<LocalTimeline>,
    remote_client_inputs: Query<
        (
            Entity,
            &Position,
            &LinearVelocity,
            &ActionState<PlayerActions>,
        ),
        With<Predicted>,
    >,
) {
    let tick = timeline.tick();
    for (entity, position, velocity, action_state) in remote_client_inputs.iter() {
        let pressed = action_state.get_pressed();
        lightyear_debug_event!(
            DebugCategory::Component,
            DebugSamplePoint::FixedUpdateBeforePhysics,
            "FixedPostUpdate",
            "player_before_physics",
            tick = ?tick,
            entity = ?entity,
            position = ?position,
            velocity = ?velocity,
            pressed = ?pressed,
            "Client in FixedPostUpdate right before physics"
        );
    }
}

fn emit_fixed_last_players(
    timeline: Res<LocalTimeline>,
    players: Query<
        (
            Entity,
            &Position,
            &LinearVelocity,
            Option<&VisualCorrection<Position>>,
            Option<&ActionState<PlayerActions>>,
            Option<&LeafwingBuffer<PlayerActions>>,
        ),
        (Without<BallMarker>, With<PlayerId>),
    >,
) {
    let tick = timeline.tick();

    for (entity, position, velocity, correction, action_state, input_buffer) in players.iter() {
        let pressed = action_state.map(|a| a.get_pressed());
        let last_buffer_tick = input_buffer.and_then(|b| b.get_last_with_tick().map(|(t, _)| t));
        lightyear_debug_event!(
            DebugCategory::Component,
            DebugSamplePoint::FixedLast,
            "FixedLast",
            "player_after_physics",
            tick = ?tick,
            entity = ?entity,
            position = ?position,
            velocity = ?velocity,
            correction = ?correction,
            pressed = ?pressed,
            last_buffer_tick = ?last_buffer_tick,
            "Player after physics update"
        );
    }
}

pub(crate) fn emit_last_players(
    timeline: Res<LocalTimeline>,
    players: Query<
        (
            Entity,
            &Position,
            Option<&VisualCorrection<Position>>,
            Option<&ActionState<PlayerActions>>,
            Option<&LeafwingBuffer<PlayerActions>>,
        ),
        (Without<BallMarker>, With<PlayerId>),
    >,
) {
    let tick = timeline.tick();

    for (entity, position, correction, action_state, input_buffer) in players.iter() {
        let pressed = action_state.map(|a| a.get_pressed());
        let last_buffer_tick = input_buffer.and_then(|b| b.get_last_with_tick().map(|(t, _)| t));
        lightyear_debug_event!(
            DebugCategory::Component,
            DebugSamplePoint::Last,
            "Last",
            "player_after_physics_last",
            tick = ?tick,
            entity = ?entity,
            position = ?position,
            correction = ?correction,
            pressed = ?pressed,
            last_buffer_tick = ?last_buffer_tick,
            "Player after physics update"
        );
    }
}

#[cfg(feature = "client")]
pub(crate) mod client {
    use super::*;

    pub(crate) fn mark_debug_players(
        mut commands: Commands,
        players: Query<Entity, (Added<PlayerId>, With<Position>)>,
    ) {
        for entity in &players {
            commands
                .entity(entity)
                .insert(LightyearDebug::component_at::<Position>([
                    DebugSamplePoint::Update,
                ]));
        }
    }

    pub(crate) fn mark_debug_balls(
        mut commands: Commands,
        balls: Query<Entity, (Added<BallMarker>, With<Position>)>,
    ) {
        for entity in &balls {
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
        players: Query<Entity, (Added<PlayerId>, With<Position>)>,
    ) {
        for entity in &players {
            commands
                .entity(entity)
                .insert(LightyearDebug::component_at::<Position>([
                    DebugSamplePoint::FixedUpdate,
                ]));
        }
    }

    pub(crate) fn mark_debug_balls(
        mut commands: Commands,
        balls: Query<Entity, (Added<BallMarker>, With<Position>)>,
    ) {
        for entity in &balls {
            commands
                .entity(entity)
                .insert(LightyearDebug::component_at::<Position>([
                    DebugSamplePoint::FixedUpdate,
                ]));
        }
    }
}
