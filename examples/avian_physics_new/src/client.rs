use avian2d::prelude::*;
use bevy::app::PluginGroupBuilder;
use bevy::prelude::*;
use core::time::Duration;
use leafwing_input_manager::prelude::*;
use lightyear::prelude::client::*;
use lightyear::prelude::*;

use crate::protocol::*;
use crate::shared;
use crate::shared::{color_from_id, shared_movement_behaviour};

pub struct ExampleClientPlugin;

impl Plugin for ExampleClientPlugin {
    fn build(&self, app: &mut App) {
        // Removed AdminActions setup
        // app.init_resource::<ActionState<AdminActions>>();
        // app.insert_resource(InputMap::<AdminActions>::new([ ... ]));

        // Removed init system
        // app.add_systems(Startup, init);
        // Removed handle_connection system
        // app.add_systems(
        //     PreUpdate,
        //     handle_connection
        //         .after(MainSet::Receive)
        //         .before(PredictionSet::SpawnPrediction),
        // );
        // all actions related-system that can be rolled back should be in FixedUpdate schedule
        app.add_systems(FixedUpdate, player_movement);
        app.add_systems(
            Update,
            (
                add_ball_physics,
                add_player_physics,
                // send_message,
                handle_predicted_spawn,
                handle_interpolated_spawn,
            ),
        );
    }
}

// Removed init system
// pub(crate) fn init(mut commands: Commands) {
//     commands.connect_client();
// }

// Removed handle_connection system (player spawning is server-authoritative)
// pub(crate) fn handle_connection( ... ) { ... }


/// Blueprint pattern: when the ball gets replicated from the server, add all the components
/// that we need that are not replicated.
/// (for example physical properties that are constant, so they don't need to be networked)
///
/// We only add the physical properties on the ball that is displayed on screen (i.e the Interpolated ball)
/// We want the ball to be rigid so that when players collide with it, they bounce off.
///
/// However we remove the Position because we want the balls position to be interpolated, without being computed/updated
/// by the physics engine? Actually this shouldn't matter because we run interpolation in PostUpdate...
fn add_ball_physics(
    mut commands: Commands,
    mut ball_query: Query<
        Entity,
        (
            With<BallMarker>,
            Or<(Added<Interpolated>, Added<Predicted>)>,
        ),
    >,
) {
    for entity in ball_query.iter_mut() {
        commands.entity(entity).insert(PhysicsBundle::ball());
    }
}

/// When we receive other players (whether they are predicted or interpolated), we want to add the physics components
/// so that our predicted entities can predict collisions with them correctly
fn add_player_physics(
    // Use LocalPlayerId resource instead of ClientConnection
    local_player_id: Option<Res<LocalPlayerId>>, // Use Option<> in case the resource isn't added yet
    mut commands: Commands,
    mut player_query: Query<
        (Entity, &PlayerId),
        (
            // insert the physics components on the player that is displayed on screen
            // (either interpolated or predicted)
            Or<(Added<Interpolated>, Added<Predicted>)>,
        ),
    >,
) {
    // Get the local client id if it exists
    let Some(local_player_id) = local_player_id else {
        warn!("LocalPlayerId resource not found, cannot add physics to remote players yet.");
        return;
    };
    let client_id = local_player_id.0;

    for (entity, player_id) in player_query.iter_mut() {
        if player_id.0 == client_id {
            // only need to do this for other players' entities
            // debug!(
            //     ?entity,
            //     ?player_id,
            //     "we only want to add physics to other player! Skip."
            // );
            continue;
        }
        info!(?entity, ?player_id, "adding physics to remote player");
        commands.entity(entity).insert(PhysicsBundle::player());
    }
}

// The client input only gets applied to predicted entities that we own
// This works because we only predict the user's controlled entity.
// If we were predicting more entities, we would have to only apply movement to the player owned one.
fn player_movement(
    tick_manager: Res<TickManager>,
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
    for (entity, player_id, position, velocity, action_state) in velocity_query.iter_mut() {
        if !action_state.get_pressed().is_empty() {
            trace!(?entity, tick = ?tick_manager.tick(), ?position, actions = ?action_state.get_pressed(), "applying movement to predicted player");
            // note that we also apply the input to the other predicted clients! even though
            //  their inputs are only replicated with a delay!
            // TODO: add input decay?
            shared_movement_behaviour(velocity, action_state);
        }
    }
}

// When the predicted copy of the client-owned entity is spawned, do stuff
// - assign it a different saturation
// - keep track of it in the Global resource
pub(crate) fn handle_predicted_spawn(mut predicted: Query<&mut ColorComponent, Added<Predicted>>) {
    for mut color in predicted.iter_mut() {
        let hsva = Hsva {
            saturation: 0.4,
            ..Hsva::from(color.0)
        };
        color.0 = Color::from(hsva);
    }
}

// When the interpolated copy of the client-owned entity is spawned, do stuff
// - assign it a different color
pub(crate) fn handle_interpolated_spawn(
    mut interpolated: Query<&mut ColorComponent, Added<Interpolated>>,
) {
    for mut color in interpolated.iter_mut() {
        let hsva = Hsva {
            saturation: 0.1,
            ..Hsva::from(color.0)
        };
        color.0 = Color::from(hsva);
    }
}
