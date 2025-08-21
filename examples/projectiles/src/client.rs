use crate::protocol::*;
use crate::shared;
use crate::shared::{color_from_id, Rooms};
use bevy::prelude::*;
use bevy_enhanced_input::action::ActionMock;
use bevy_enhanced_input::bindings;
use core::time::Duration;
use lightyear::input::bei::prelude::*;
use lightyear::input::client::InputSet;
use lightyear::prelude::client::*;
use lightyear::prelude::*;

pub struct ExampleClientPlugin;

impl Plugin for ExampleClientPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            PreUpdate,
            // mock the action before BEI evaluates it. BEI evaluated actions mocks in FixedPreUpdate
            update_cursor_state_from_window,
        );
        app.add_observer(handle_predicted_spawn);
        app.add_observer(handle_interpolated_spawn);
        app.add_observer(add_client_actions);
        // app.add_observer(cycle_projectile_mode);
        // app.add_observer(cycle_replication_mode);
    }
}

/// Compute the world-position of the cursor and set it in the DualAxis input
fn update_cursor_state_from_window(
    window: Single<&Window>,
    q_camera: Query<(&Camera, &GlobalTransform)>,
    mut action_query: Query<&mut ActionMock, With<Action<MoveCursor>>>,
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
        for mut action_mock in action_query.iter_mut() {
            action_mock.value = ActionValue::Axis2D(world_position);
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
        commands.entity(trigger.target()).insert(PlayerContext);
        commands.spawn((
            ActionOf::<PlayerContext>::new(trigger.target()),
            Action::<MovePlayer>::new(),
            Bindings::spawn(Cardinal::wasd_keys()),
        ));
        commands.spawn((
            ActionOf::<PlayerContext>::new(trigger.target()),
            Action::<MoveCursor>::new(),
            ActionMock::new(
                ActionState::Fired,
                ActionValue::zero(ActionValueDim::Axis2D),
                MockSpan::Manual,
            ),
            InputMarker::<PlayerContext>::default(),
        ));
        commands.spawn((
            ActionOf::<PlayerContext>::new(trigger.target()),
            Action::<Shoot>::new(),
            Bindings::spawn_one((Binding::from(KeyCode::Space), Name::from("Binding"))),
        ));
        commands.spawn((
            ActionOf::<PlayerContext>::new(trigger.target()),
            Action::<CycleWeapon>::new(),
            Bindings::spawn_one((Binding::from(KeyCode::KeyQ), Name::from("Binding"))),
        ));
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

pub(crate) fn add_client_actions(trigger: Trigger<OnAdd, Client>, mut commands: Commands) {
    // the context needs to be added on both client and server
    commands.entity(trigger.target()).insert(ClientContext);
    commands.spawn((
        ActionOf::<ClientContext>::new(trigger.target()),
        Action::<CycleProjectileMode>::new(),
        bindings![KeyCode::KeyE,],
    ));
    commands.spawn((
        ActionOf::<ClientContext>::new(trigger.target()),
        Action::<CycleReplicationMode>::new(),
        bindings![KeyCode::KeyR,],
    ));
}

// pub fn cycle_replication_mode(
//     trigger: Trigger<Fired<CycleReplicationMode>>,
//     client: Single<&mut GameReplicationMode, With<Client>>,
// ) {
//     let mut replication_mode = client.into_inner();
//     *replication_mode = replication_mode.next();
// }
//
// pub fn cycle_projectile_mode(
//     trigger: Trigger<Fired<CycleProjectileMode>>,
//     client: Single<&mut ProjectileReplicationMode, With<Client>>,
// ) {
//     let mut projectile_mode = client.into_inner();
//     *projectile_mode = projectile_mode.next();
// }
