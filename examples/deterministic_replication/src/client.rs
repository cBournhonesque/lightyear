use avian2d::parry::shape::Ball;
use avian2d::prelude::*;
use bevy::app::PluginGroupBuilder;
use bevy::prelude::*;
use core::time::Duration;
use leafwing_input_manager::prelude::*;
use lightyear::input::leafwing::prelude::LeafwingBuffer;
use lightyear::prediction::predicted_history::PredictionHistory;
use lightyear::prediction::rollback::{DeterministicPredicted, DisableRollback};
use lightyear::prelude::client::*;
use lightyear::prelude::*;

use crate::automation::AutomationClientPlugin;
use crate::protocol::*;
use crate::shared::{self, GameStartMode, color_from_id, player_bundle};

pub struct ExampleClientPlugin;

impl Plugin for ExampleClientPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(AutomationClientPlugin);
        if !app
            .is_plugin_added::<lightyear_deterministic_replication::prelude::ChecksumSendPlugin>()
        {
            app.add_plugins(lightyear_deterministic_replication::prelude::ChecksumSendPlugin);
        }
        app.add_systems(
            PreUpdate,
            add_input_map_after_sync.after(ReplicationSystems::Receive),
        );
        app.add_systems(
            FixedPreUpdate,
            extend_input_buffers_for_late_join
                .before(lightyear::input::client::InputSystems::BufferClientInputs),
        );
        app.add_systems(
            FixedPreUpdate,
            (apply_ball_snapshot, activate_physics_at_tick).chain(),
        );
    }
}

#[derive(Component)]
struct InputMapAdded;

#[derive(Resource)]
struct BallSnapshotApplied;

fn add_input_map_after_sync(
    client: Option<Single<&LocalId, (With<Client>, With<IsSynced<InputTimeline>>)>>,
    mut commands: Commands,
    players: Query<(Entity, &PlayerId), (Without<InputMapAdded>, Without<InputMap<PlayerActions>>)>,
) {
    let Some(client) = client else {
        return;
    };
    let local_id = client.into_inner();
    for (entity, player_id) in players.iter() {
        if local_id.0 == player_id.0 {
            info!("Client: adding InputMap to local player {:?}", player_id.0);
            commands.entity(entity).insert((
                InputMap::new([
                    (PlayerActions::Up, KeyCode::KeyW),
                    (PlayerActions::Down, KeyCode::KeyS),
                    (PlayerActions::Left, KeyCode::KeyA),
                    (PlayerActions::Right, KeyCode::KeyD),
                ]),
                InputMapAdded,
            ));
        }
    }
}

fn apply_ball_snapshot(
    mut commands: Commands,
    timeline: Res<LocalTimeline>,
    applied: Option<Res<BallSnapshotApplied>>,
    ball_state: Query<&BallPhysicsState>,
    mut ball: Query<(&mut Position, &mut LinearVelocity, &mut AngularVelocity), With<BallMarker>>,
    pending_players: Query<&PhysicsStartTick, Without<DeterministicPredicted>>,
) {
    if applied.is_some() {
        return;
    }
    let tick = timeline.tick();
    let has_late_join = pending_players.iter().any(|start| tick > start.0);
    if !has_late_join {
        return;
    }
    let Ok(state) = ball_state.single() else {
        return;
    };
    let Ok((mut pos, mut lin_vel, mut ang_vel)) = ball.single_mut() else {
        return;
    };
    info!(
        "Client: late-join — applying ball snapshot: pos={:?}",
        state.position
    );
    pos.0 = state.position;
    lin_vel.0 = state.linear_velocity;
    ang_vel.0 = state.angular_velocity;
    commands.insert_resource(BallSnapshotApplied);
}

fn extend_input_buffers_for_late_join(
    timeline: Res<LocalTimeline>,
    mut buffers: Query<
        (
            &PlayerId,
            &PhysicsStartTick,
            &mut LeafwingBuffer<PlayerActions>,
        ),
        Without<DeterministicPredicted>,
    >,
) {
    let tick = timeline.tick();
    for (player_id, start, mut buffer) in buffers.iter_mut() {
        if tick > start.0 {
            if let Some(last) = buffer.get_last().cloned() {
                info!(
                    "Client: extending input buffer for player {:?} to tick {:?}, last input: {:?}",
                    player_id.0, tick, last
                );
                buffer.set(tick, last);
            }
        }
    }
}

fn activate_physics_at_tick(
    client: Option<Single<&LocalId, (With<Client>, With<IsSynced<InputTimeline>>)>>,
    timeline: Res<LocalTimeline>,
    mut commands: Commands,
    pending: Query<
        (
            Entity,
            &PlayerId,
            &PhysicsStartTick,
            Option<&PlayerPhysicsState>,
        ),
        Without<DeterministicPredicted>,
    >,
) {
    let Some(client) = client else {
        return;
    };
    let local_id = client.into_inner();
    let tick = timeline.tick();
    for (entity, player_id, start, physics_state) in pending.iter() {
        if tick >= start.0 {
            let late_join = tick > start.0;
            info!(
                "Client: activating physics for player {:?} at tick {:?} (scheduled {:?}, late_join={})",
                player_id.0, tick, start.0, late_join
            );
            let mut entity_mut = commands.entity(entity);
            entity_mut.insert((
                PhysicsBundle::player(),
                ColorComponent(color_from_id(player_id.0)),
                Name::from("Player"),
                DeterministicPredicted {
                    skip_despawn: true,
                    ..default()
                },
            ));
            if late_join {
                if let Some(state) = physics_state {
                    info!(
                        "Client: late-join — using replicated state for player {:?}: pos={:?}, vel={:?}",
                        player_id.0, state.position, state.linear_velocity
                    );
                    entity_mut.insert((
                        Position::from(state.position),
                        Rotation::radians(state.rotation),
                        LinearVelocity(state.linear_velocity),
                        AngularVelocity(state.angular_velocity),
                    ));
                } else {
                    let y = (player_id.0.to_bits() as f32 * 50.0) % 500.0 - 250.0;
                    entity_mut.insert(Position::from(Vec2::new(-50.0, y)));
                }
            } else {
                let y = (player_id.0.to_bits() as f32 * 50.0) % 500.0 - 250.0;
                entity_mut.insert(Position::from(Vec2::new(-50.0, y)));
            }
            if local_id.0 == player_id.0 {
                entity_mut.insert(InputMap::new([
                    (PlayerActions::Up, KeyCode::KeyW),
                    (PlayerActions::Down, KeyCode::KeyS),
                    (PlayerActions::Left, KeyCode::KeyA),
                    (PlayerActions::Right, KeyCode::KeyD),
                ]));
            }
        }
    }
}
