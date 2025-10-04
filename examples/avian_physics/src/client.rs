use avian2d::prelude::*;
use bevy::app::PluginGroupBuilder;
use bevy::prelude::*;
use core::time::Duration;
use leafwing_input_manager::prelude::*;
use lightyear::connection::host::HostClient;
use lightyear::prelude::client::*;
use lightyear::prelude::*;

use crate::protocol::*;
use crate::shared;
use crate::shared::{color_from_id, shared_movement_behaviour, SharedPlugin};

pub struct ExampleClientPlugin;

impl Plugin for ExampleClientPlugin {
    fn build(&self, app: &mut App) {
        // all actions related-system that can be rolled back should be in FixedUpdate schedule
        app.add_systems(FixedUpdate, player_movement);
        app.add_observer(add_ball_physics);
        app.add_observer(handle_interpolated_spawn);
        app.add_observer(handle_predicted_spawn);

        // DEBUG
        app.add_systems(PostUpdate, print_overstep);
    }
}

/// Blueprint pattern: when the ball gets replicated from the server, add all the components
/// that we need that are not replicated.
/// (for example physical properties that are constant, so they don't need to be networked)
///
/// We only add the physical properties on the ball that is displayed on screen (i.e the Predicted ball)
/// We want the ball to be rigid so that when players collide with it, they bounce off.
///
/// However we remove the Position because we want the balls position to be interpolated, without being computed/updated
/// by the physics engine? Actually this shouldn't matter because we run interpolation in PostUpdate...
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

// The client input only gets applied to predicted entities that we own
// This works because we only predict the user's controlled entity.
// If we were predicting more entities, we would have to only apply movement to the player owned one.
fn player_movement(
    // In host-server mode, the players are already moved by the server system so we don't want
    // to move them twice.
    timeline: Single<&LocalTimeline, (With<Client>, Without<HostClient>)>,
    mut velocity_query: Query<
        (
            Entity,
            &PlayerId,
            &Position,
            &mut LinearVelocity,
            &ActionState<PlayerActions>,
        ),
        With<Predicted>,
    >,
) {
    let tick = timeline.tick();
    for (entity, player_id, position, velocity, action_state) in velocity_query.iter_mut() {
        if !action_state.get_pressed().is_empty() {
            trace!(?entity, ?tick, ?position, actions = ?action_state.get_pressed(), "applying movement to predicted player");
            // note that we also apply the input to the other predicted clients! even though
            //  their inputs are only replicated with a delay!
            // TODO: add input decay?
            shared_movement_behaviour(velocity, action_state);
        }
    }
}

// When the predicted copy of the client-owned entity is spawned, do stuff
// - assign it a different saturation
// - add physics components so that its movement can be predicted
pub(crate) fn handle_predicted_spawn(
    trigger: On<Add, (PlayerId, Predicted)>,
    mut commands: Commands,
    mut player_query: Query<(&mut ColorComponent, Has<Controlled>), With<Predicted>>,
) {
    if let Ok((mut color, controlled)) = player_query.get_mut(trigger.entity) {
        let hsva = Hsva {
            saturation: 0.4,
            ..Hsva::from(color.0)
        };
        color.0 = Color::from(hsva);
        let mut entity_mut = commands.entity(trigger.entity);
        entity_mut.insert(PhysicsBundle::player());
        if controlled {
            entity_mut.insert(InputMap::new([
                (PlayerActions::Up, KeyCode::KeyW),
                (PlayerActions::Down, KeyCode::KeyS),
                (PlayerActions::Left, KeyCode::KeyA),
                (PlayerActions::Right, KeyCode::KeyD),
            ]));
        }
    }
}

// When the interpolated copy of the client-owned entity is spawned, do stuff
// - assign it a different color
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

// Debug system to check on the oversteps
fn print_overstep(time: Res<Time<Fixed>>, timeline: Single<&InputTimeline, With<Client>>) {
    let input_overstep = timeline.overstep();
    let input_overstep_ms = input_overstep.value() * (time.timestep().as_millis() as f32);
    let time_overstep = time.overstep();
    trace!(?input_overstep_ms, ?time_overstep, "overstep");
}
