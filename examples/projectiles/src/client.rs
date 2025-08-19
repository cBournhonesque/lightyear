use bevy::prelude::*;
use core::time::Duration;
use leafwing_input_manager::action_state::ActionData;
use leafwing_input_manager::buttonlike::ButtonState::Pressed;
use leafwing_input_manager::plugin::InputManagerSystem;
use leafwing_input_manager::prelude::*;
use lightyear::input::client::InputSet;
use lightyear::prelude::client::*;
use lightyear::prelude::*;
use lightyear::prelude::input::native;
use crate::protocol::*;
use crate::shared;
use crate::shared::{color_from_id, shared_player_movement, Rooms};

pub struct ExampleClientPlugin;

impl Plugin for ExampleClientPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            FixedPreUpdate,
            update_cursor_state_from_window
                // make sure that we update the ActionState before we buffer it in the InputBuffer
                .before(InputSet::BufferClientInputs)
                .in_set(InputManagerSystem::ManualControl),
        );
        app.add_observer(handle_predicted_spawn);
        app.add_observer(handle_interpolated_spawn);
    }
}

/// Compute the world-position of the cursor and set it in the DualAxis input
fn update_cursor_state_from_window(
    window: Single<&Window>,
    q_camera: Query<(&Camera, &GlobalTransform)>,
    mut action_state_query: Query<&mut ActionState<PlayerActions>, With<Predicted>>,
) {
    let Ok((camera, camera_transform)) = q_camera.single() else {
        error!("Expected to find only one camera");
        return;
    };
    if let Some(world_position) = window
        .cursor_position()
        .and_then(|cursor| Some(camera.viewport_to_world(camera_transform, cursor).unwrap()))
        .map(|ray| ray.origin.truncate())
    {
        for mut action_state in action_state_query.iter_mut() {
            action_state.set_axis_pair(&PlayerActions::MoveCursor, world_position);
        }
    }
}

// When the predicted copy of the client-owned entity is spawned, do stuff
// - assign it a different saturation
// - add physics components so that its movement can be predicted
pub(crate) fn handle_predicted_spawn(
    trigger: Trigger<OnAdd, (PlayerId, Predicted)>,
    mut commands: Commands,
    mut player_query: Query<&mut ColorComponent, With<Predicted>>,
) {
    if let Ok(mut color) = player_query.get_mut(trigger.target()) {
        let hsva = Hsva {
            saturation: 0.4,
            ..Hsva::from(color.0)
        };
        color.0 = Color::from(hsva);
        commands.entity(trigger.target()).insert((InputMap::new([
            (PlayerActions::Up, KeyCode::KeyW),
            (PlayerActions::Down, KeyCode::KeyS),
            (PlayerActions::Left, KeyCode::KeyA),
            (PlayerActions::Right, KeyCode::KeyD),
            (PlayerActions::Shoot, KeyCode::Space),
            (PlayerActions::CycleWeapon, KeyCode::KeyQ),
        ]),));
    }
}

pub(crate) fn handle_interpolated_spawn(
    trigger: Trigger<OnAdd, ColorComponent>,
    mut interpolated: Query<&mut ColorComponent, Added<Interpolated>>,
) {
    if let Ok(mut color) = interpolated.get_mut(trigger.target()) {
        let hsva = Hsva {
            saturation: 0.1,
            ..Hsva::from(color.0)
        };
        color.0 = Color::from(hsva);
    }
}

// TODO: ReplicationMetadata sends Client<>ClientOf mapping on connection
pub(crate) fn display_mode(
    trigger: Trigger<OnAdd, Client>,
    mut commands: Commands,
) {
    // commands.entity(trigger.target())
    //     .insert(native::InputMarker::new([
    //         (ExampleActions::CycleReplicationMode, KeyCode::KeyE),
    //         (ExampleActions::CycleRoom, KeyCode::KeyR),
    //     ]));
}

pub(crate) fn mode_cycling(
    client: Single<(&mut GameReplicationMode, &mut ProjectileReplicationMode, &native::ActionState<ExampleActions>), With<Client>>,
) {
    for (mut replication_mode, mut projectile_mode, action) in client.iter_mut() {
        if action.just_pressed(&ExampleActions::CycleReplicationMode) {
            *replication_mode = replication_mode.next();
        }
        if action.just_pressed(&ExampleActions::CycleProjectileMode) {
            *projectile_mode = projectile_mode.next();
        }
    }
}