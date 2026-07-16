use avian2d::prelude::*;
use bevy::app::PluginGroupBuilder;
use bevy::prelude::*;
use core::time::Duration;
use leafwing_input_manager::prelude::*;
use lightyear::connection::host::HostClient;
use lightyear::prelude::client::*;
use lightyear::prelude::*;
use lightyear_frame_interpolation::{FrameInterpolate, FrameInterpolationPlugin};

use crate::automation::AutomationClientPlugin;
use crate::protocol::*;
use crate::shared;
use crate::shared::{shared_movement_behaviour, PlayerChildCollider, SharedPlugin};

pub struct ExampleClientPlugin;

impl Plugin for ExampleClientPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(AutomationClientPlugin);
        // Keep the visual pipeline available in headless clients so prediction,
        // frame interpolation, and correction can be tested without a window.
        if !app.is_plugin_added::<FrameInterpolationPlugin>() {
            app.add_plugins(FrameInterpolationPlugin);
        }
        // Rollback-capable action systems must run in FixedUpdate.
        app.add_systems(FixedUpdate, player_movement);
        app.add_observer(add_ball_physics);
        app.add_observer(handle_interpolated_spawn);
        app.add_observer(handle_predicted_spawn);
        app.add_observer(add_frame_interpolation_components);
        app.add_observer(handle_controlled_spawn);

        app.add_systems(PostUpdate, crate::debug::print_overstep);
    }
}

/// Blueprint pattern: when the ball gets replicated from the server, add all the components
/// that we need that are not replicated.
/// (for example physical properties that are constant, so they don't need to be networked)
///
/// We only add the physical properties on the ball that is displayed on screen (i.e the Predicted ball)
/// We want the ball to be rigid so that when players collide with it, they bounce off.
///
/// The replicated `Position` remains authoritative; this adds only the local physics data needed
/// for predicted collision response.
fn add_ball_physics(
    trigger: On<Add, BallMarker>,
    mut commands: Commands,
    ball_query: Query<(), With<Predicted>>,
) {
    if let Ok(()) = ball_query.get(trigger.entity) {
        commands
            .entity(trigger.entity)
            .insert(PhysicsBundle::ball());
    }
}

// Apply predicted inputs only to player rigid-body roots. Child colliders do not
// have PlayerId, LinearVelocity, or ActionState, so input cannot move them independently.
fn player_movement(
    // In host-server mode, the players are already moved by the server system so we don't want
    // to move them twice.
    timeline: Res<LocalTimeline>,
    mut velocity_query: Query<
        (
            Entity,
            &Position,
            &mut LinearVelocity,
            &ActionState<PlayerActions>,
        ),
        (With<Predicted>, With<PlayerId>),
    >,
) {
    let tick = timeline.tick();
    for (entity, position, velocity, action_state) in velocity_query.iter_mut() {
        if !action_state.get_pressed().is_empty() {
            trace!(?entity, ?tick, ?position, actions = ?action_state.get_pressed(), "applying movement to predicted player");
            shared_movement_behaviour(velocity, action_state);
        }
    }
}

/// Predicted physics is updated at fixed ticks; interpolate its Position and Rotation
/// for rendering between those ticks and for post-rollback visual correction.
fn add_frame_interpolation_components(
    trigger: On<Add, (Position, RigidBody, Predicted)>,
    query: Query<Entity, (With<Predicted>, With<RigidBody>)>,
    mut commands: Commands,
) {
    if query.contains(trigger.entity) {
        commands.entity(trigger.entity).insert(FrameInterpolate);
    }
}

// Prepare predicted player entities for local simulation and distinguish them visually.
pub(crate) fn handle_predicted_spawn(
    trigger: On<Add, (PlayerId, Predicted)>,
    mut commands: Commands,
    mut player_query: Query<&mut ColorComponent, With<Predicted>>,
) {
    if let Ok(mut color) = player_query.get_mut(trigger.entity) {
        let hsva = Hsva {
            saturation: 0.4,
            ..Hsva::from(color.0)
        };
        color.0 = Color::from(hsva);
        commands
            .entity(trigger.entity)
            .insert(PhysicsBundle::player());
    }
}

fn handle_controlled_spawn(
    trigger: On<Add, Controlled>,
    mut commands: Commands,
    player_query: Query<&PlayerId, Without<InputMap<PlayerActions>>>,
) {
    if player_query.get(trigger.entity).is_err() {
        return;
    };
    commands.entity(trigger.entity).insert(InputMap::new([
        (PlayerActions::Up, KeyCode::KeyW),
        (PlayerActions::Down, KeyCode::KeyS),
        (PlayerActions::Left, KeyCode::KeyA),
        (PlayerActions::Right, KeyCode::KeyD),
    ]));
}

// Lower the saturation on interpolated entities so they are visually distinct.
pub(crate) fn handle_interpolated_spawn(
    trigger: On<Add, ColorComponent>,
    mut interpolated: Query<&mut ColorComponent, Added<Interpolated>>,
) {
    if let Ok(mut color) = interpolated.get_mut(trigger.entity) {
        let hsva = Hsva {
            saturation: 0.1,
            ..Hsva::from(color.0)
        };
        color.0 = Color::from(hsva);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_interpolation_covers_the_predicted_ball_but_not_non_rigid_children() {
        let mut app = App::new();
        app.add_observer(add_frame_interpolation_components);

        let ball = app
            .world_mut()
            .spawn((Position::default(), RigidBody::Dynamic, Predicted))
            .id();
        let child = app
            .world_mut()
            .spawn((Position::default(), Predicted, PlayerChildCollider))
            .id();
        app.update();

        assert!(
            app.world().get::<FrameInterpolate>(ball).is_some(),
            "a predicted ball is a physics root even though it has no PlayerId"
        );
        assert!(
            app.world().get::<FrameInterpolate>(child).is_none(),
            "a non-rigid child has no fixed-tick pose of its own to frame-interpolate"
        );
    }
}
