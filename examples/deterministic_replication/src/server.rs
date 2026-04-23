use crate::automation::AutomationServerPlugin;
use crate::protocol::*;
use crate::shared::{
    self, GameStartMode, SharedPlugin, WallBundle, color_from_id, player_bundle,
    shared_movement_behaviour,
};
use avian2d::prelude::*;
use bevy::color::palettes::css;
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use core::time::Duration;
use leafwing_input_manager::prelude::*;
use lightyear::input::leafwing::prelude::LeafwingBuffer;
use lightyear::prelude::server::*;
use lightyear::prelude::*;
use lightyear_examples_common::shared::SEND_INTERVAL;

/// How many ticks in the future to schedule physics start, giving clients
/// time to receive the `PhysicsStartTick` component via replication.
const START_TICK_BUFFER: i32 = 20;

#[derive(Clone)]
pub struct ExampleServerPlugin;

impl Plugin for ExampleServerPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(AutomationServerPlugin);
        if !app
            .is_plugin_added::<lightyear_deterministic_replication::prelude::ChecksumReceivePlugin>(
            )
        {
            app.add_plugins(lightyear_deterministic_replication::prelude::ChecksumReceivePlugin);
        }
        app.insert_resource(ReplicationMetadata::new(SEND_INTERVAL));
        app.add_observer(handle_new_client);
        app.add_observer(handle_connected);
        app.add_systems(
            FixedPreUpdate,
            (schedule_physics_start, activate_physics_at_tick).chain(),
        );
        app.add_systems(
            FixedPostUpdate,
            (sync_player_physics_state, sync_ball_physics_state)
                .run_if(resource_equals(GameStartMode::Flexible)),
        );
        app.add_systems(
            Startup,
            spawn_ball_state_entity.run_if(resource_equals(GameStartMode::Flexible)),
        );
    }
}

pub(crate) fn handle_new_client(trigger: On<Add, LinkOf>, mut commands: Commands) {
    commands.entity(trigger.entity).insert(ReplicationSender);
}

pub(crate) fn handle_connected(
    trigger: On<Add, Connected>,
    query: Query<&RemoteId, With<ClientOf>>,
    game_mode: Res<GameStartMode>,
    mut commands: Commands,
) {
    let Ok(remote_id) = query.get(trigger.entity) else {
        return;
    };
    info!("Spawning player entity for client {:?}", remote_id);
    let mut entity = commands.spawn((
        Replicate::to_clients(NetworkTarget::All),
        PlayerId(remote_id.0),
        WaitingForFirstInput,
    ));
    if matches!(*game_mode, GameStartMode::Flexible) {
        entity.insert(PlayerPhysicsState::default());
    }
}

#[derive(Component)]
pub struct WaitingForFirstInput;

/// When required players have sent their first input, schedule a future
/// start tick and replicate `PhysicsStartTick` to all player entities.
fn schedule_physics_start(
    mut commands: Commands,
    waiting: Query<(Entity, &PlayerId), With<WaitingForFirstInput>>,
    buffers: Query<&LeafwingBuffer<PlayerActions>>,
    game_start_mode: Res<GameStartMode>,
    timeline: Res<LocalTimeline>,
) {
    let ready_entities: Vec<(Entity, &PlayerId)> = match game_start_mode.as_ref() {
        GameStartMode::Flexible => waiting
            .iter()
            .filter(|(entity, _)| buffers.get(*entity).is_ok())
            .collect(),
        GameStartMode::AllReady { num_players } => {
            let count = waiting.iter().count();
            if count < *num_players {
                return;
            }
            if !waiting
                .iter()
                .all(|(entity, _)| buffers.get(entity).is_ok())
            {
                return;
            }
            waiting.iter().collect()
        }
    };

    if ready_entities.is_empty() {
        return;
    }

    let start_tick = timeline.tick() + START_TICK_BUFFER;
    info!(
        "Server: scheduling physics start at tick {:?} for {} player(s)",
        start_tick,
        ready_entities.len()
    );
    for (entity, _player_id) in ready_entities {
        commands.entity(entity).remove::<WaitingForFirstInput>();
        commands.entity(entity).insert(PhysicsStartTick(start_tick));
    }
}

/// When the current tick reaches the scheduled start tick, add physics.
fn activate_physics_at_tick(
    mut commands: Commands,
    timeline: Res<LocalTimeline>,
    pending: Query<(Entity, &PlayerId, &PhysicsStartTick), Without<Position>>,
) {
    let tick = timeline.tick();
    for (entity, player_id, start) in pending.iter() {
        if tick >= start.0 {
            info!(
                "Server: activating physics for player {:?} at tick {:?}",
                player_id.0, tick
            );
            commands.entity(entity).insert(player_bundle(player_id.0));
        }
    }
}

/// Mirror server-side physics into the replicated snapshot so late-joiners
/// receive up-to-date state.
fn sync_player_physics_state(
    timeline: Res<LocalTimeline>,
    mut players: Query<(
        &mut PlayerPhysicsState,
        &Position,
        &Rotation,
        &LinearVelocity,
        &AngularVelocity,
    )>,
) {
    let tick = timeline.tick();
    for (mut state, pos, rot, lin_vel, ang_vel) in players.iter_mut() {
        let new_state = PlayerPhysicsState {
            tick,
            position: pos.0,
            rotation: rot.as_radians(),
            linear_velocity: lin_vel.0,
            angular_velocity: ang_vel.0,
        };
        if *state != new_state {
            *state = new_state;
        }
    }
}

fn spawn_ball_state_entity(mut commands: Commands) {
    commands.spawn((
        Replicate::to_clients(NetworkTarget::All),
        BallPhysicsState::default(),
    ));
}

fn sync_ball_physics_state(
    timeline: Res<LocalTimeline>,
    ball: Query<(&Position, &LinearVelocity, &AngularVelocity), With<BallMarker>>,
    mut state_query: Query<&mut BallPhysicsState>,
) {
    let tick = timeline.tick();
    let Ok((pos, lin_vel, ang_vel)) = ball.single() else {
        return;
    };
    let Ok(mut state) = state_query.single_mut() else {
        return;
    };
    let new_state = BallPhysicsState {
        tick,
        position: pos.0,
        linear_velocity: lin_vel.0,
        angular_velocity: ang_vel.0,
    };
    if *state != new_state {
        *state = new_state;
    }
}
