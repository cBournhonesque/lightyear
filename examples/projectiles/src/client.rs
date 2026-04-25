use crate::automation::AutomationClientPlugin;
use crate::protocol::*;
use crate::shared;
use crate::shared::color_from_id;
use avian2d::prelude::*;
use bevy::prelude::*;
use bevy_enhanced_input::bindings;
use core::time::Duration;
use lightyear::input::bei::prelude::*;
use lightyear::input::client::InputSystems;
use lightyear::prediction::rollback::DisableRollback;
use lightyear::prelude::client::*;
use lightyear::prelude::*;

pub struct ExampleClientPlugin;

impl Plugin for ExampleClientPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(AutomationClientPlugin);
        app.add_observer(handle_predicted_spawn);
        app.add_observer(handle_interpolated_spawn);
        app.add_observer(handle_deterministic_spawn);
        app.add_observer(add_global_actions);
        // app.add_observer(cycle_projectile_mode);
        // app.add_observer(cycle_replication_mode);
        // app.add_systems(RunFixedMainLoop, cycle_replication_mode.in_set(RunFixedMainLoopSystems::BeforeFixedMainLoop), "gui");
        // app.add_systems(FixedUpdate, cycle_replication_mode_fixed_update);
    }
}

// When the predicted copy of the client-owned entity is spawned, do stuff
// - assign it a different saturation
// - add physics components so that its movement can be predicted
pub(crate) fn handle_predicted_spawn(
    trigger: On<Add, (PlayerMarker, Predicted)>,
    client: Single<&LocalId, With<Client>>,
    mut commands: Commands,
    mut player_query: Query<(&PlayerId, &Position, &GameReplicationMode), With<Predicted>>,
) {
    let client_id = client.into_inner().0;
    if let Ok((player_id, pos, mode)) = player_query.get_mut(trigger.entity) {
        if mode == &GameReplicationMode::AllInterpolated {
            return;
        };
        match mode {
            GameReplicationMode::ClientSideHitDetection
            | GameReplicationMode::OnlyInputsReplicated => {
                // add these so we can do hit-detection on the client
                commands.entity(trigger.entity).insert((
                    Collider::rectangle(PLAYER_SIZE, PLAYER_SIZE),
                    RigidBody::Kinematic,
                ));
            }
            _ => {}
        };
        if player_id.0 != client_id {
            return;
        }
        info!(
            ?pos,
            "Adding actions to predicted player {:?}", trigger.entity
        );
        // add actions on the local entity (remote predicted entities will have actions propagated by the server)
        shared::spawn_player_actions(&mut commands, trigger.entity, player_id.0, *mode, false);
    }
}

pub(crate) fn handle_interpolated_spawn(
    trigger: On<Add, (PlayerMarker, Interpolated)>,
    client: Single<&LocalId, With<Client>>,
    mut interpolated: Query<
        (&PlayerId, &Interpolated, &GameReplicationMode),
        (With<Interpolated>, With<PlayerMarker>),
    >,
    mut commands: Commands,
) {
    let client_id = client.into_inner();
    if let Ok((player_id, interpolated, mode)) = interpolated.get_mut(trigger.entity) {
        if mode == &GameReplicationMode::ClientSideHitDetection {
            // add these so we can do hit-detection on the client
            commands
                .entity(trigger.entity)
                .insert((Collider::rectangle(PLAYER_SIZE, PLAYER_SIZE),));
        }
        // In the interpolated case, the client sends inputs but doesn't apply them.
        // Only the server applies the inputs, and the position changes are replicated back
        if let GameReplicationMode::AllInterpolated = mode
            && client_id.0 == player_id.0
        {
            shared::spawn_player_actions(&mut commands, trigger.entity, player_id.0, *mode, false);
        }
    }
}

pub(crate) fn handle_deterministic_spawn(
    trigger: On<Add, PlayerMarker>,
    query: Query<(&PlayerId, &GameReplicationMode)>,
    client: Single<&LocalId, With<Client>>,
    mut commands: Commands,
) {
    let client_id = client.into_inner();
    if let Ok((player_id, mode)) = query.get(trigger.entity)
        && mode == &GameReplicationMode::OnlyInputsReplicated
    {
        commands.entity(trigger.entity).insert((
            shared::player_bundle(player_id.0, GameReplicationMode::OnlyInputsReplicated),
            DeterministicPredicted {
                // make sure that we don't despawn the player if there is a rollback
                skip_despawn: true,
                ..default()
            },
        ));
        info!("Adding PlayerContext for player {:?}", player_id);

        // add actions for the local client
        if player_id.0 == client_id.0 {
            info!(
                "Spawning actions for DeterministicPredicted player {:?}",
                player_id
            );
            shared::spawn_player_actions(&mut commands, trigger.entity, player_id.0, *mode, false);
        }
    }
}

pub(crate) fn add_global_actions(trigger: On<Add, ClientContext>, mut commands: Commands) {
    commands.spawn((
        ActionOf::<ClientContext>::new(trigger.entity),
        Action::<CycleProjectileMode>::new(),
        bindings![KeyCode::KeyE,],
    ));
    commands.spawn((
        ActionOf::<ClientContext>::new(trigger.entity),
        Action::<CycleReplicationMode>::new(),
        bindings![KeyCode::KeyR,],
    ));
    commands.spawn((
        ActionOf::<ClientContext>::new(trigger.entity),
        Action::<CycleWeapon>::new(),
        bindings![KeyCode::KeyQ,],
    ));
}

pub fn cycle_replication_mode(
    timeline: Res<LocalTimeline>,
    action: Single<(Entity, &ActionValue, &ActionEvents), With<Action<CycleReplicationMode>>>,
) {
    let tick = timeline.tick();
    let (entity, action_value, action_events) = action.into_inner();
    trace!(
        ?tick,
        ?entity,
        "CycleReplicationMode PreUpdate action value: {:?}, events: {:?}",
        action_value,
        action_events
    );
}

pub fn cycle_replication_mode_fixed_update(
    timeline: Res<LocalTimeline>,
    action: Single<(Entity, &ActionValue, &ActionEvents), With<Action<CycleReplicationMode>>>,
) {
    let tick = timeline.tick();
    let (entity, action_value, action_events) = action.into_inner();
    trace!(
        ?tick,
        ?entity,
        "CycleReplicationMode FixedUpdate action value: {:?}, events: {:?}",
        action_value,
        action_events
    );
}
