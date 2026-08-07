//! Deterministic input-only P2P setup for the spaceships demo.

use core::f32::consts::TAU;

use avian2d::prelude::*;
use bevy::color::palettes::css;
use bevy::prelude::*;
use lightyear::prediction::rollback::DeterministicPredicted;
use lightyear::prelude::input::client::InputSystems;
use lightyear::prelude::input::leafwing::LeafwingBuffer;
use lightyear::prelude::*;
use lightyear_deterministic_replication::prelude::DeterministicReplicationPlugin;
use lightyear_examples_common::p2p::{
    GAMEPLAY_START_TICK, P2PGameplayStarted, P2PSettings, input_target_for_peer,
};
use lightyear_examples_common::shared::FIXED_TIMESTEP_HZ;
use lightyear_frame_interpolation::FrameInterpolate;

use crate::client::player_input_map;
use crate::protocol::*;
use crate::shared::color_from_id;

const PLAYER_INPUT_HASH_BASE: u64 = 0x5350_4143_4500_0000;

pub struct ExampleP2PPlugin;

impl Plugin for ExampleP2PPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(PredictionManager::default());
        app.add_plugins(DeterministicReplicationPlugin);
        app.add_systems(
            FixedPreUpdate,
            spawn_fixed_world.before(InputSystems::BufferClientInputs),
        );
        app.add_systems(
            FixedPostUpdate,
            update_scores.after(crate::shared::process_collisions),
        );
    }
}

fn spawn_fixed_world(
    mut commands: Commands,
    _synced: SyncedInputTimeline,
    timeline: Res<LocalTimeline>,
    started: Option<Res<P2PGameplayStarted>>,
    settings: Res<P2PSettings>,
    links: Query<(Entity, &RemoteId), With<P2P>>,
) {
    if started.is_some() || timeline.tick().0 < GAMEPLAY_START_TICK {
        return;
    }
    commands.insert_resource(P2PGameplayStarted);

    const NUM_BALLS: usize = 6;
    for i in 0..NUM_BALLS {
        let radius = 10.0 + i as f32 * 4.0;
        let angle = i as f32 * (TAU / NUM_BALLS as f32);
        let marker = BallMarker::new(radius);
        commands.spawn((
            Position(Vec2::new(125.0 * angle.cos(), 125.0 * angle.sin())),
            ColorComponent(css::GOLD.into()),
            marker.physics_bundle(),
            marker,
            DeterministicPredicted {
                skip_despawn: true,
                enable_rollback_after: 0,
            },
            FrameInterpolate,
            Name::new("P2P Ball"),
        ));
    }

    for peer_id in settings.peer_ids() {
        let id = PeerId::Entity(u64::from(peer_id));
        let angle = f32::from(peer_id) * (TAU / f32::from(settings.player_count));
        let target = input_target_for_peer(
            &settings,
            &links,
            peer_id,
            PLAYER_INPUT_HASH_BASE | u64::from(peer_id),
        );
        let player = commands
            .spawn((
                Player::new(id, format!("Peer {peer_id}")),
                Score(0),
                Position(Vec2::new(200.0 * angle.cos(), 200.0 * angle.sin())),
                PhysicsBundle::player_ship(),
                Weapon::new((FIXED_TIMESTEP_HZ / 5.0) as u16),
                ColorComponent(color_from_id(id)),
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
            commands.entity(player).insert(player_input_map());
        }
    }
}

fn update_scores(
    mut events: MessageReader<BulletHitMessage>,
    mut players: Query<(&Player, &mut Score)>,
) {
    for event in events.read() {
        let Some(victim) = event.victim_client_id else {
            continue;
        };
        for (player, mut score) in &mut players {
            if player.client_id == victim {
                score.0 -= 1;
            }
            if player.client_id == event.bullet_owner {
                score.0 += 1;
            }
        }
    }
}
