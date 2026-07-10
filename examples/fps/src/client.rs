use avian2d::prelude::RigidBody;
use bevy::prelude::*;
use core::time::Duration;
use leafwing_input_manager::action_state::ActionData;
use leafwing_input_manager::buttonlike::ButtonState::Pressed;
use leafwing_input_manager::plugin::InputManagerSystem;
use leafwing_input_manager::prelude::*;
use lightyear::connection::host::HostServer;
use lightyear::input::client::InputSystems;
use lightyear::prelude::client::*;
use lightyear::prelude::*;

use crate::automation::AutomationClientPlugin;
use crate::protocol::*;
use crate::shared;
use crate::shared::{color_from_id, shared_player_movement};
use lightyear_frame_interpolation::FrameInterpolate;

pub struct ExampleClientPlugin;

impl Plugin for ExampleClientPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(AutomationClientPlugin);
        app.add_systems(
            FixedPreUpdate,
            (update_cursor_state_from_window, suppress_shoot_until_synced)
                .chain()
                // make sure that we update the ActionState before we buffer it in the InputBuffer
                .before(InputSystems::BufferClientInputs)
                .in_set(InputManagerSystem::ManualControl),
        );
        app.add_systems(
            PreUpdate,
            strip_interpolated_bullet_local_physics.after(ReplicationSystems::Receive),
        );
        app.add_observer(handle_predicted_spawn);
        app.add_observer(handle_controlled_spawn);
        app.add_observer(handle_interpolated_spawn);
    }
}

fn strip_interpolated_bullet_local_physics(
    mut commands: Commands,
    bullets: Query<
        Entity,
        (
            With<Interpolated>,
            // Host-client authoritative bullets can also carry client markers.
            Without<Replicate>,
            // BulletMarker is delayed onto the interpolation timeline, but once its history exists
            // this entity is already known to be a remote bullet and must not run local physics.
            Or<(With<BulletMarker>, With<ConfirmedHistory<BulletMarker>>)>,
            Or<(With<RigidBody>, With<FrameInterpolate>)>,
        ),
    >,
) {
    for entity in &bullets {
        commands
            .entity(entity)
            .remove::<(RigidBody, FrameInterpolate)>();
    }
}

fn suppress_shoot_until_synced(
    host_server: Query<(), With<HostServer>>,
    synced_client: Query<(), (With<Client>, With<IsSynced<InputTimeline>>)>,
    mut action_state_query: Query<&mut ActionState<PlayerActions>, With<Predicted>>,
) {
    if !host_server.is_empty() || !synced_client.is_empty() {
        return;
    }
    for mut action_state in &mut action_state_query {
        action_state.release(&PlayerActions::Shoot);
    }
}

/// Compute the world-position of the cursor and set it in the DualAxis input
fn update_cursor_state_from_window(
    window: Option<Single<&Window>>,
    q_camera: Query<(&Camera, &GlobalTransform)>,
    mut action_state_query: Query<&mut ActionState<PlayerActions>, With<Predicted>>,
) {
    if automation_aim_target_enabled() {
        return;
    }
    let Some(window) = window else {
        return;
    };
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

fn automation_aim_target_enabled() -> bool {
    #[cfg(not(target_family = "wasm"))]
    {
        std::env::var("LIGHTYEAR_AIM_TARGET")
            .map(|value| {
                let value = value.trim().to_ascii_lowercase();
                !value.is_empty() && value != "any"
            })
            .unwrap_or(false)
    }
    #[cfg(target_family = "wasm")]
    {
        false
    }
}

// Lower the saturation on predicted entities so they are visually distinct.
pub(crate) fn handle_predicted_spawn(
    trigger: On<Add, (PlayerId, Predicted)>,
    mut player_query: Query<&mut ColorComponent, With<Predicted>>,
) {
    if let Ok(mut color) = player_query.get_mut(trigger.entity) {
        let hsva = Hsva {
            saturation: 0.4,
            ..Hsva::from(color.0)
        };
        color.0 = Color::from(hsva);
    }
}

/// Add local input bindings once ownership is known.
///
/// `Predicted` and `Controlled` can arrive in either order, especially in host-client mode. The
/// input map is tied to local ownership, so key it off `Controlled` instead of prediction timing.
pub(crate) fn handle_controlled_spawn(
    trigger: On<Add, Controlled>,
    mut commands: Commands,
    player_query: Query<(), (With<PlayerMarker>, Without<InputMap<PlayerActions>>)>,
) {
    let entity = trigger.entity;
    if player_query.get(entity).is_err() {
        return;
    };
    commands.entity(entity).insert(InputMap::new([
        (PlayerActions::Up, KeyCode::KeyW),
        (PlayerActions::Down, KeyCode::KeyS),
        (PlayerActions::Left, KeyCode::KeyA),
        (PlayerActions::Right, KeyCode::KeyD),
        (PlayerActions::Shoot, KeyCode::Space),
    ]));
}

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
