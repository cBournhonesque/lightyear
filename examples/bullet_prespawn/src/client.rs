use bevy::prelude::*;
use bevy::utils::Duration;
use leafwing_input_manager::action_state::ActionData;
use leafwing_input_manager::buttonlike::ButtonState::Pressed;
use leafwing_input_manager::plugin::InputManagerSystem;
use leafwing_input_manager::prelude::*;
use lightyear::client::input::leafwing::InputSystemSet;
use lightyear::inputs::native::input_buffer::InputBuffer;
use lightyear::prelude::client::*;
use lightyear::prelude::*;

use crate::protocol::*;
use crate::shared;
use crate::shared::{color_from_id, shared_player_movement};

pub struct ExampleClientPlugin;

impl Plugin for ExampleClientPlugin {
    fn build(&self, app: &mut App) {
        // To send global inputs, insert the ActionState and the InputMap as Resources
        app.init_resource::<ActionState<AdminActions>>();
        app.insert_resource(InputMap::<AdminActions>::new([
            (AdminActions::SendMessage, KeyCode::KeyM),
            (AdminActions::Reset, KeyCode::KeyR),
        ]));

        // all actions related-system that can be rolled back should be in the `FixedUpdate` schdule
        // app.add_systems(FixedUpdate, player_movement);
        // we update the ActionState manually from cursor, so we need to put it in the ManualControl set

        // NOTE: we need to run this in FixedUpdate because we generate the ActionDiffs in
        // FixedUpdate using the fixed_update value
        app.add_systems(
            FixedPreUpdate,
            update_cursor_state_from_window
                // make sure that we update the ActionState before we buffer it in the InputBuffer
                .before(InputSystemSet::BufferClientInputs)
                .in_set(InputManagerSystem::ManualControl),
        );
        app.add_systems(
            PreUpdate,
            (
                // TODO: make sure it happens after update metadata?
                spawn_player,
            ),
        );
        app.add_systems(Update, (handle_predicted_spawn, handle_interpolated_spawn));
    }
}

fn spawn_player(mut commands: Commands, mut connection_event: EventReader<ConnectEvent>) {
    for event in connection_event.read() {
        let client_id = event.client_id();
        commands.spawn((
            Text(format!("Client {}", client_id)),
            TextColor(Color::WHITE),
            TextFont::default().with_font_size(30.0),
        ));
        info!("Spawning player with id: {}", client_id);
        let y = (client_id.to_bits() as f32 * 50.0) % 500.0 - 250.0;
        commands.spawn(PlayerBundle::new(
            client_id,
            Vec2::new(-50.0, y),
            InputMap::new([
                (PlayerActions::Up, KeyCode::KeyW),
                (PlayerActions::Down, KeyCode::KeyS),
                (PlayerActions::Left, KeyCode::KeyA),
                (PlayerActions::Right, KeyCode::KeyD),
                (PlayerActions::Shoot, KeyCode::Space),
            ]),
        ));
    }
}

// Compute the world-position of the cursor
fn update_cursor_state_from_window(
    window_query: Query<&Window>,
    // query to get camera transform
    q_camera: Query<(&Camera, &GlobalTransform)>,
    mut action_state_query: Query<&mut ActionState<PlayerActions>, With<Predicted>>,
) {
    let Ok((camera, camera_transform)) = q_camera.get_single() else {
        error!("Expected to find only one camera");
        return;
    };
    let window = window_query.single();
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

// Get the cursor position relative to the window
fn window_relative_mouse_position(window: &Window) -> Option<Vec2> {
    let cursor_pos = window.cursor_position()?;
    Some(Vec2::new(
        cursor_pos.x - (window.width() / 2.0),
        (cursor_pos.y - (window.height() / 2.0)) * -1.0,
    ))
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

// When the predicted copy of the client-owned entity is spawned, do stuff
// - assign it a different saturation
// - keep track of it in the Global resource
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
