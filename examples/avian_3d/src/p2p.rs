//! Deterministic input-only P2P setup for the Avian 3D character example.

use avian3d::prelude::*;
use bevy::prelude::*;
use leafwing_input_manager::prelude::ActionState;
use lightyear::prediction::rollback::DeterministicPredicted;
use lightyear::prelude::input::client::InputSystems;
use lightyear::prelude::input::leafwing::LeafwingBuffer;
use lightyear::prelude::*;
use lightyear_deterministic_replication::prelude::DeterministicReplicationPlugin;
use lightyear_examples_common::p2p::{
    input_target_for_peer, insert_example_session, P2PGameplayStarted, P2PSettings,
};

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

/// Marks the local character until its input map can be enabled after the first physics snapshot.
#[derive(Component)]
struct PendingLocalInput(Tick);

pub struct ExampleP2PPlugin;

impl Plugin for ExampleP2PPlugin {
    fn build(&self, app: &mut App) {
        insert_example_session(app, PLAYER_INPUT_HASH_BASE);
        app.insert_resource(PredictionManager::default());
        app.add_plugins(DeterministicReplicationPlugin);
        app.add_systems(
            FixedPreUpdate,
            spawn_fixed_world.before(InputSystems::BufferClientInputs),
        );
        app.add_systems(
            FixedPostUpdate,
            enable_local_input.after(PredictionSystems::UpdateHistory),
        );
        app.add_systems(FixedUpdate, (shoot, despawn_projectiles));
    }
}

fn spawn_fixed_world(
    mut commands: Commands,
    session: Res<P2PSession>,
    started: Option<Res<P2PGameplayStarted>>,
    settings: Res<P2PSettings>,
    links: Query<(Entity, &RemoteId), With<P2P>>,
) {
    if started.is_some() || !session.is_running() {
        return;
    }
    commands.insert_resource(P2PGameplayStarted);

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
    let center = (f32::from(settings.player_count) - 1.0) * 0.5;
    for peer_id in settings.peer_ids() {
        let id = PeerId::Entity(u64::from(peer_id));
        let target = input_target_for_peer(
            &settings,
            &links,
            peer_id,
            PLAYER_INPUT_HASH_BASE | u64::from(peer_id),
        );
        let character = commands
            .spawn((
                Name::new("P2P Character"),
                Position(Vec3::new((f32::from(peer_id) - center) * spacing, 3.0, 0.0)),
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
        if peer_id == settings.local_peer_id {
            commands
                .entity(character)
                .insert(PendingLocalInput(timeline.tick()));
        }
    }
}

/// Enable input capture only after Avian and Lightyear have recorded the world's initial state.
///
/// The fixed world and its collider-tree proxies are created together at
/// [`GAMEPLAY_START_TICK`]. Enabling input in the same tick would allow a late first input to roll
/// back before the first complete physics snapshot, leaving live colliders paired with an empty
/// restored collider tree. Waiting one complete warm-up tick makes the earliest possible input
/// rollback target a world snapshot from after initialization. It also lets the per-collider
/// rollback histories installed by Avian's observers record their initial proxy state.
fn enable_local_input(
    mut commands: Commands,
    timeline: Res<LocalTimeline>,
    local_characters: Query<(Entity, &PendingLocalInput)>,
) {
    for (entity, pending) in &local_characters {
        if timeline.tick() <= pending.0 {
            continue;
        }
        commands
            .entity(entity)
            .insert(character_input_map())
            .remove::<PendingLocalInput>();
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
