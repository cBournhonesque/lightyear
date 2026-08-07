//! Deterministic input-only P2P setup for the FPS example.
//!
//! Server-only lag compensation is intentionally absent. Both bots, every player, projectiles,
//! scores, and hit prediction instead live in the same deterministic simulation on every peer.

use avian2d::prelude::*;
use bevy::prelude::*;
use lightyear::prediction::rollback::DeterministicPredicted;
use lightyear::prelude::input::leafwing::LeafwingBuffer;
use lightyear::prelude::*;
use lightyear_deterministic_replication::prelude::DeterministicReplicationPlugin;
use lightyear_examples_common::p2p::{input_target_for_peer, P2PSettings};
use lightyear_frame_interpolation::FrameInterpolate;

use crate::client::player_input_map;
use crate::protocol::*;
use crate::shared::{color_from_id, BOT_RADIUS};

const PLAYER_INPUT_HASH_BASE: u64 = 0x4650_5300_0000_0000;

#[derive(Component)]
struct PendingLocalInput(Tick);

pub struct ExampleP2PPlugin;

impl Plugin for ExampleP2PPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(PredictionManager::default());
        app.add_plugins(DeterministicReplicationPlugin);
        app.add_observer(spawn_fixed_world);
        app.add_systems(
            FixedPostUpdate,
            enable_local_input.after(PredictionSystems::UpdateHistory),
        );

        app.add_systems(
            FixedPostUpdate,
            compute_hits.after(PhysicsSystems::StepSimulation),
        );
    }
}

fn compute_hits(
    mut commands: Commands,
    spatial_query: SpatialQuery,
    bullets: Query<(Entity, &PlayerId, &Position, &LinearVelocity), With<BulletMarker>>,
    bots: Query<(), With<PredictedBot>>,
    mut players: Query<(&mut Score, &PlayerId), With<PlayerMarker>>,
) {
    const COLLISION_DISTANCE: f32 = 4.0;
    for (entity, shooter, position, velocity) in &bullets {
        let Some(_hit) = spatial_query.cast_ray_predicate(
            position.0,
            Dir2::new_unchecked(velocity.0.normalize()),
            COLLISION_DISTANCE,
            false,
            &SpatialQueryFilter::default(),
            &|entity| bots.get(entity).is_ok(),
        ) else {
            continue;
        };
        if let Some((mut score, _)) = players.iter_mut().find(|(_, id)| id.0 == shooter.0) {
            score.0 += 1;
        }
        commands.entity(entity).prediction_despawn();
    }
}

fn spawn_fixed_world(
    _trigger: On<P2PStarted>,
    mut commands: Commands,
    timeline: Res<LocalTimeline>,
    settings: Res<P2PSettings>,
    links: Query<(Entity, &RemoteId), With<P2P>>,
) {
    commands.spawn((
        PredictedBot,
        Name::new("P2P Predicted Bot"),
        Position::from_xy(200.0, 10.0),
        Rotation::default(),
        RigidBody::Kinematic,
        Collider::circle(BOT_RADIUS),
        DeterministicPredicted {
            skip_despawn: true,
            enable_rollback_after: 0,
        },
        FrameInterpolate,
    ));
    commands.spawn((
        InterpolatedBot,
        Name::new("P2P Second Bot"),
        Position::from_xy(-200.0, 10.0),
        Rotation::default(),
        RigidBody::Kinematic,
        Collider::circle(BOT_RADIUS),
        DeterministicPredicted {
            skip_despawn: true,
            enable_rollback_after: 0,
        },
        FrameInterpolate,
    ));

    let spacing = 100.0;
    let center = (f32::from(settings.player_count) - 1.0) * 0.5;
    for peer_id in settings.peer_ids() {
        let id = PeerId::Entity(u64::from(peer_id));
        let target = input_target_for_peer(
            &settings,
            &links,
            peer_id,
            PLAYER_INPUT_HASH_BASE | u64::from(peer_id),
        );
        let player = commands
            .spawn((
                Score(0),
                PlayerId(id),
                RigidBody::Kinematic,
                Position::from_xy(0.0, (f32::from(peer_id) - center) * spacing),
                Rotation::default(),
                ColorComponent(color_from_id(id)),
                PlayerMarker,
                DeterministicPredicted {
                    skip_despawn: true,
                    enable_rollback_after: 0,
                },
                target,
                LeafwingBuffer::<PlayerActions>::default(),
                FrameInterpolate,
                Name::new("P2P Player"),
            ))
            .id();
        if peer_id == settings.local_peer_id {
            commands
                .entity(player)
                .insert(PendingLocalInput(timeline.tick()));
        }
    }
}

/// Enable local input after the initial Avian rollback state has been recorded.
fn enable_local_input(
    mut commands: Commands,
    timeline: Res<LocalTimeline>,
    local_players: Query<(Entity, &PendingLocalInput)>,
) {
    for (entity, pending) in &local_players {
        if timeline.tick() <= pending.0 {
            continue;
        }
        commands
            .entity(entity)
            .insert(player_input_map())
            .remove::<PendingLocalInput>();
    }
}
