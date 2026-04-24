use avian2d::prelude::*;
use bevy::prelude::*;
use leafwing_input_manager::prelude::*;
use lightyear::input::leafwing::prelude::LeafwingBuffer;
use lightyear::prediction::predicted_history::PredictionHistory;
use lightyear::prediction::rollback::DeterministicPredicted;
use lightyear::prelude::client::*;
use lightyear::prelude::*;
use lightyear_replication::checkpoint::ReplicationCheckpointMap;
use lightyear_replication::prelude::ConfirmHistory;

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
        app.add_systems(FixedPreUpdate, activate_physics_at_tick);
        app.add_systems(
            FixedPostUpdate,
            seed_confirmed_history
                .run_if(resource_equals(GameStartMode::Flexible)),
        );
    }
}

#[derive(Component)]
struct InputMapAdded;

#[derive(Component)]
struct PhysicsActivated;

/// Marker: confirmed history has been seeded from the replicate_once value.
#[derive(Component)]
struct ConfirmedHistorySeeded;

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

/// When a replicate_once Position arrives on an entity, write it (and the
/// other physics components) into PredictionHistory as confirmed state at the
/// server tick that produced the value. This lets input-triggered rollbacks
/// snap to the correct initial state for late-joined entities.
/// After activation adds DeterministicPredicted (which creates PredictionHistory),
/// write the replicate_once Position/Velocity into PredictionHistory as confirmed
/// state at the correct server tick. This lets input-triggered rollbacks snap to
/// the exact server state for late-joined entities.
fn seed_confirmed_history(
    checkpoints: Res<ReplicationCheckpointMap>,
    mut commands: Commands,
    mut entities: Query<
        (
            Entity,
            &Position,
            &Rotation,
            &LinearVelocity,
            &AngularVelocity,
            &ConfirmHistory,
            &mut PredictionHistory<Position>,
            &mut PredictionHistory<Rotation>,
            &mut PredictionHistory<LinearVelocity>,
            &mut PredictionHistory<AngularVelocity>,
        ),
        (
            With<DeterministicPredicted>,
            Without<ConfirmedHistorySeeded>,
        ),
    >,
) {
    for (
        entity,
        pos,
        rot,
        lin_vel,
        ang_vel,
        confirm,
        mut pos_hist,
        mut rot_hist,
        mut lv_hist,
        mut av_hist,
    ) in entities.iter_mut()
    {
        let Some(tick) = checkpoints.get(confirm.last_tick()) else {
            continue;
        };
        info!(
            "Client: seeding confirmed history at tick {:?} for entity {:?}: pos={:?}",
            tick, entity, pos.0
        );
        pos_hist.add_confirmed(tick, Some(pos.clone()));
        rot_hist.add_confirmed(tick, Some(rot.clone()));
        lv_hist.add_confirmed(tick, Some(lin_vel.clone()));
        av_hist.add_confirmed(tick, Some(ang_vel.clone()));
        commands.entity(entity).insert(ConfirmedHistorySeeded);
    }
}

fn activate_physics_at_tick(
    client: Option<Single<&LocalId, (With<Client>, With<IsSynced<InputTimeline>>)>>,
    timeline: Res<LocalTimeline>,
    mut commands: Commands,
    pending: Query<
        (Entity, &PlayerId, &PhysicsStartTick, Option<&Position>),
        Without<PhysicsActivated>,
    >,
) {
    let Some(client) = client else {
        return;
    };
    let local_id = client.into_inner();
    let tick = timeline.tick();
    for (entity, player_id, start, existing_position) in pending.iter() {
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
                PhysicsActivated,
            ));
            // For on-time activation, set Position from the spawn formula.
            // For late-join, Position already arrived via replicate_once.
            if !late_join || existing_position.is_none() {
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
