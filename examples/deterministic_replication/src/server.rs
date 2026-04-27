use crate::automation::AutomationServerPlugin;
use crate::protocol::*;
use crate::shared::{GameStartMode, player_bundle};
use avian2d::prelude::*;
use bevy::prelude::*;
use lightyear::input::leafwing::prelude::LeafwingBuffer;
use lightyear::prediction::rollback::DeterministicPredicted;
use lightyear::prelude::server::*;
use lightyear::prelude::*;
use lightyear_deterministic_replication::prelude::{AppCatchUpExt, CatchUpGated};
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
        // The LateJoinCatchUpPlugin itself is added by ProtocolPlugin
        // (in SharedPlugin) so it runs before `cli.spawn_connections`.
        // Here we register which components are catch-up-gated: they
        // are hidden from each client by default and only sent once
        // after the client explicitly requests catch-up (see
        // `lightyear_deterministic_replication::late_join`).
        app.register_catchup_components::<(Position, Rotation, LinearVelocity, AngularVelocity)>();
        app.insert_resource(ReplicationMetadata::new(SEND_INTERVAL));
        app.add_observer(handle_new_client);
        app.add_observer(handle_connected);
        app.add_systems(
            FixedPreUpdate,
            (schedule_physics_start, activate_physics_at_tick).chain(),
        );
    }
}

pub(crate) fn handle_new_client(trigger: On<Add, LinkOf>, mut commands: Commands) {
    commands.entity(trigger.entity).insert(ReplicationSender);
}

pub(crate) fn handle_connected(
    trigger: On<Add, Connected>,
    query: Query<&RemoteId, With<ClientOf>>,
    mut commands: Commands,
) {
    let Ok(remote_id) = query.get(trigger.entity) else {
        return;
    };
    info!("Spawning player entity for client {:?}", remote_id);
    commands.spawn((
        Replicate::to_clients(NetworkTarget::All),
        PlayerId(remote_id.0),
        WaitingForFirstInput,
        // CatchUpGated: hide registered physics components from every client
        // until that client sends `CatchUpForEntity`. Structural components
        // (PlayerId, DeterministicPredicted, PhysicsStartTick) are still
        // replicated normally, so clients know the entity exists and can
        // subscribe to input rebroadcasts for it.
        CatchUpGated,
    ));
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
            // `DeterministicPredicted` pulls in the `Deterministic` marker
            // (via `register_required_components`), which is how the
            // checksum system identifies which entities to hash. The
            // client inserts the same marker when it activates physics;
            // adding it here keeps the server hashing the same *set* of
            // entities as the clients.
            commands.entity(entity).insert((
                player_bundle(player_id.0),
                DeterministicPredicted {
                    skip_despawn: false,
                    ..default()
                },
            ));
        }
    }
}
