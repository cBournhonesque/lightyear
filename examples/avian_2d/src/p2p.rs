//! Deterministic input-only P2P setup for the Avian 2D example.

use avian2d::prelude::*;
use bevy::color::palettes::css;
use bevy::prelude::*;
use leafwing_input_manager::prelude::ActionState;
use lightyear::prediction::rollback::DeterministicPredicted;
use lightyear::prelude::input::leafwing::LeafwingBuffer;
use lightyear::prelude::*;
use lightyear_deterministic_replication::prelude::DeterministicReplicationPlugin;
use lightyear_examples_common::p2p::{input_target_for_peer, P2PSettings};
use lightyear_frame_interpolation::FrameInterpolate;

use crate::client::player_input_map;
use crate::protocol::*;
use crate::shared::color_from_id;

const PLAYER_INPUT_HASH_BASE: u64 = 0x4156_3244_0000_0000;

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
        Position::default(),
        ColorComponent(css::AZURE.into()),
        PhysicsBundle::ball(),
        BallMarker,
        DeterministicPredicted {
            skip_despawn: true,
            enable_rollback_after: 0,
        },
        FrameInterpolate,
        Name::from("P2P Ball"),
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
                PlayerId(id),
                Position::from(Vec2::new(-50.0, (f32::from(peer_id) - center) * spacing)),
                Rotation::radians(0.15),
                AngularVelocity(0.35),
                ColorComponent(color_from_id(id)),
                PhysicsBundle::player(),
                DeterministicPredicted {
                    skip_despawn: true,
                    enable_rollback_after: 0,
                },
                target,
                LeafwingBuffer::<PlayerActions>::default(),
                FrameInterpolate,
                Name::from("P2P Player"),
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
