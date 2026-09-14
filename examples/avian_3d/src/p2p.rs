//! Deterministic input-only P2P setup for the Avian 3D character example.

use avian3d::prelude::*;
use bevy::prelude::*;
use leafwing_input_manager::prelude::ActionState;
use lightyear::p2p::Lobby;
use lightyear::prediction::rollback::DeterministicPredicted;
use lightyear::prelude::input::leafwing::LeafwingBuffer;
use lightyear::prelude::*;
use lightyear_deterministic_replication::prelude::DeterministicReplicationPlugin;
use lightyear_examples_common::p2p::input_target_for_peer;

use crate::client::character_input_map;
use crate::protocol::*;
use crate::shared::{
    color_from_id, BlockPhysicsBundle, CharacterPhysicsBundle, FloorPhysicsBundle,
    ProjectilePhysicsBundle,
};

const PLAYER_INPUT_HASH_BASE: u64 = 0x4156_3344_0000_0000;
const PROJECTILE_LIFETIME_TICKS: i32 = 120;

#[derive(Component)]
struct DespawnAt(Tick);

pub struct ExampleP2PPlugin;

impl Plugin for ExampleP2PPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(PredictionManager::default());
        app.add_plugins(DeterministicReplicationPlugin);
        app.add_observer(spawn_fixed_world);
        app.add_systems(FixedUpdate, (shoot, despawn_projectiles));
    }
}

fn spawn_fixed_world(
    _trigger: On<P2PStarted>,
    mut commands: Commands,
    lobby: Res<Lobby>,
    links: Query<(Entity, &RemoteId), With<P2P>>,
) {
    commands.spawn((
        Name::new("P2P Floor"),
        FloorPhysicsBundle::default(),
        FloorMarker,
        Position::new(Vec3::ZERO),
    ));
    commands.spawn((
        Name::new("P2P Block"),
        BlockPhysicsBundle::default(),
        BlockMarker,
        Position::new(Vec3::new(1.0, 1.0, 0.0)),
        DeterministicPredicted {
            skip_despawn: true,
            enable_rollback_after: 0,
        },
    ));

    let spacing = 2.0;
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
        let character = commands
            .spawn((
                Name::new("P2P Character"),
                Position(Vec3::new((f32::from(slot) - center) * spacing, 3.0, 0.0)),
                CharacterPhysicsBundle::default(),
                ColorComponent(color_from_id(id)),
                CharacterMarker,
                DeterministicPredicted {
                    skip_despawn: true,
                    enable_rollback_after: 0,
                },
                target,
                LeafwingBuffer::<CharacterAction>::default(),
            ))
            .id();
        if Some(*peer) == local {
            commands.entity(character).insert(character_input_map());
        }
    }
}

fn shoot(
    mut commands: Commands,
    timeline: Res<LocalTimeline>,
    players: Query<(&ActionState<CharacterAction>, &Position), With<CharacterMarker>>,
) {
    let tick = timeline.tick();
    for (action, position) in &players {
        if !action.just_pressed(&CharacterAction::Shoot) {
            continue;
        }
        commands.spawn((
            Name::new("P2P Projectile"),
            ProjectileMarker,
            ProjectilePhysicsBundle::default(),
            *position,
            Rotation::default(),
            LinearVelocity(Vec3::Z * 10.0),
            DespawnAt(tick + PROJECTILE_LIFETIME_TICKS),
            DeterministicPredicted::default(),
        ));
    }
}

fn despawn_projectiles(
    mut commands: Commands,
    timeline: Res<LocalTimeline>,
    projectiles: Query<(Entity, &DespawnAt)>,
) {
    let tick = timeline.tick();
    for (entity, despawn) in &projectiles {
        if tick >= despawn.0 {
            commands.entity(entity).prediction_despawn();
        }
    }
}
