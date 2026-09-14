//! Deterministic input-only P2P setup for the FPS example.
//!
//! Server-only lag compensation is intentionally absent. Both bots, every player, projectiles,
//! scores, and hit prediction instead live in the same deterministic simulation on every peer.

use avian2d::prelude::*;
use bevy::prelude::*;
use lightyear::p2p::Lobby;
use lightyear::prediction::rollback::DeterministicPredicted;
use lightyear::prelude::input::leafwing::LeafwingBuffer;
use lightyear::prelude::*;
use lightyear_deterministic_replication::prelude::DeterministicReplicationPlugin;
use lightyear_examples_common::p2p::input_target_for_peer;
use lightyear_frame_interpolation::FrameInterpolate;

use crate::client::player_input_map;
use crate::protocol::*;
use crate::shared::{color_from_id, BOT_RADIUS};

const PLAYER_INPUT_HASH_BASE: u64 = 0x4650_5300_0000_0000;

pub struct ExampleP2PPlugin;

impl Plugin for ExampleP2PPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(PredictionManager::default());
        app.add_plugins(DeterministicReplicationPlugin);
        app.add_observer(spawn_fixed_world);

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
    lobby: Res<Lobby>,
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
    let roster = lobby.roster();
    let local = lobby.local();
    let center = (roster.len() as f32 - 1.0) * 0.5;
    for (slot, peer) in roster.iter().enumerate() {
        let slot = u8::try_from(slot).expect("P2P roster slot fits in u8");
        let id = PeerId::Entity(u64::from(slot));
        let target = input_target_for_peer(
            &lobby,
            &links,
            *peer,
            PLAYER_INPUT_HASH_BASE | u64::from(slot),
        );
        let player = commands
            .spawn((
                Score(0),
                PlayerId(id),
                RigidBody::Kinematic,
                Position::from_xy(0.0, (f32::from(slot) - center) * spacing),
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
        if Some(*peer) == local {
            commands.entity(player).insert(player_input_map());
        }
    }
}
