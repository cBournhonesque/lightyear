use crate::protocol::*;
use crate::shared;
use crate::shared::{color_from_id, Rooms};
use bevy::prelude::*;
use bevy_enhanced_input::action::ActionMock;
use bevy_enhanced_input::bindings;
use lightyear::prediction::rollback::DisableRollback;
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
        app.add_observer(handle_deterministic_spawn);
        app.add_observer(add_client_actions);
        // app.add_observer(cycle_projectile_mode);
        // app.add_observer(cycle_replication_mode);
        app.add_systems(RunFixedMainLoop, cycle_replication_mode.in_set(RunFixedMainLoopSystem::BeforeFixedMainLoop));
        app.add_systems(FixedUpdate, cycle_replication_mode_fixed_update);
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
    mut player_query: Query<(&mut ColorComponent, &GameReplicationMode), With<Predicted>>,
) {
    if let Ok((mut color, replication)) = player_query.get_mut(trigger.target()) {
        let hsva = Hsva {
            saturation: 0.4,
            ..Hsva::from(color.0)
        };
        color.0 = Color::from(hsva);
        match replication {
            GameReplicationMode::AllInterpolated => {},
            _ => {
                add_actions(&mut commands, trigger.target());
            }
        }
    }
}

pub(crate) fn handle_interpolated_spawn(
    trigger: Trigger<OnAdd, ColorComponent>,
    mut interpolated: Query<(&mut ColorComponent, &GameReplicationMode, &Interpolated), Added<Interpolated>>,
    mut commands: Commands,
) {
    if let Ok((mut color, replication_mode, interpolated)) = interpolated.get_mut(trigger.target()) {
        let hsva = Hsva {
            saturation: 0.1,
            ..Hsva::from(color.0)
        };
        color.0 = Color::from(hsva);
        // In the interpolated case, the client controls the confirmed entity
        if let GameReplicationMode::AllInterpolated = replication_mode {
            add_actions(&mut commands, interpolated.confirmed_entity);
        }
    }
}

pub(crate) fn handle_deterministic_spawn(
    trigger: Trigger<OnAdd, GameReplicationMode>,
    client: Single<&LocalId, With<Client>>,
    query: Query<&GameReplicationMode>,
    mut commands: Commands,
) {
    // TODO: it's controllable by the client, but it doesn't render!
    let client_id = client.into_inner();
    if let Ok(replication_mode) = query.get(trigger.target()) {
        match replication_mode {
            GameReplicationMode::OnlyInputsReplicated => {
                commands.entity(trigger.target()).insert((
                    shared::player_bundle(client_id.0),
                    DeterministicPredicted
                ));
                add_actions(&mut commands, trigger.target());
            },
            _ => {
                // handled in the predicted/interpolated spawn handlers
            }
        }
    }
}

fn add_actions(
    commands: &mut Commands,
    player: Entity,
) {
    commands.entity(player).insert(PlayerContext);
    commands.spawn((
        ActionOf::<PlayerContext>::new(player),
        Action::<MovePlayer>::new(),
        Bindings::spawn(Cardinal::wasd_keys()),
    ));
    commands.spawn((
        ActionOf::<PlayerContext>::new(player),
        Action::<MoveCursor>::new(),
        ActionMock::new(
            ActionState::Fired,
            ActionValue::zero(ActionValueDim::Axis2D),
            MockSpan::Manual,
        ),
        InputMarker::<PlayerContext>::default(),
    ));
    commands.spawn((
        ActionOf::<PlayerContext>::new(player),
        Action::<Shoot>::new(),
        Bindings::spawn_one((Binding::from(KeyCode::Space), Name::from("Binding"))),
    ));
    commands.spawn((
        ActionOf::<PlayerContext>::new(player),
        Action::<CycleWeapon>::new(),
        Bindings::spawn_one((Binding::from(KeyCode::KeyQ), Name::from("Binding"))),
    ));
}

// Add components on the Client entity when it connects
pub(crate) fn add_client_actions(
    trigger: Trigger<OnAdd, Connected>,
    mut commands: Commands
) {
    // the context needs to be added on both client and server
    commands.entity(trigger.target()).insert((
        ClientContext,
        ProjectileReplicationMode::default(),
        GameReplicationMode::default(),
    ));
    // TODO: we should have a way to spawn Inputs/Actions in Update, where they are not affected by rollback!
    commands.spawn((
        ActionOf::<ClientContext>::new(trigger.target()),
        Action::<CycleProjectileMode>::new(),
        // we don't want to retrigger this action during rollback
        DisableRollback,
        bindings![KeyCode::KeyE,],
    ));
    commands.spawn((
        ActionOf::<ClientContext>::new(trigger.target()),
        Action::<CycleReplicationMode>::new(),
        // we don't want to retrigger this action during rollback
        DisableRollback,
        bindings![KeyCode::KeyR,],
    ));
}

pub fn cycle_replication_mode(
    timeline: Single<(&LocalTimeline, Has<Rollback>)>,
    action: Single<(Entity, &ActionValue, &ActionEvents), With<Action<CycleReplicationMode>>>,
) {
    let (timeline, rollback) = timeline.into_inner();
    let tick = timeline.tick();
    let (entity, action_value, action_events) = action.into_inner();
    trace!(?tick, ?rollback, ?entity, "CycleReplicationMode PreUpdate action value: {:?}, events: {:?}", action_value, action_events);
}

pub fn cycle_replication_mode_fixed_update(
    timeline: Single<(&LocalTimeline, Has<Rollback>)>,
    action: Single<(Entity, &ActionValue, &ActionEvents), With<Action<CycleReplicationMode>>>,
) {
    let (timeline, rollback) = timeline.into_inner();
    let tick = timeline.tick();
    let (entity, action_value, action_events) = action.into_inner();
    trace!(?tick, ?rollback, ?entity, "CycleReplicationMode FixedUpdate action value: {:?}, events: {:?}", action_value, action_events);
}