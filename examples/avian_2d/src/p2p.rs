//! Deterministic input-only P2P setup for the Avian 2D example.

use avian2d::prelude::*;
use bevy::color::palettes::css;
use bevy::prelude::*;
use leafwing_input_manager::prelude::ActionState;
use lightyear::p2p::Lobby;
use lightyear::prediction::rollback::DeterministicPredicted;
use lightyear::prelude::input::leafwing::LeafwingBuffer;
use lightyear::prelude::*;
use lightyear_deterministic_replication::prelude::DeterministicReplicationPlugin;
use lightyear_examples_common::p2p::input_target_for_peer;
use lightyear_frame_interpolation::FrameInterpolate;

use crate::client::player_input_map;
use crate::protocol::*;
use crate::shared::color_from_id;

const PLAYER_INPUT_HASH_BASE: u64 = 0x4156_3244_0000_0000;

pub struct ExampleP2PPlugin;

impl Plugin for ExampleP2PPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(PredictionManager::default());
        app.add_plugins(DeterministicReplicationPlugin);
        app.add_observer(spawn_fixed_world);
    }
}

fn spawn_fixed_world(
    _trigger: On<P2PStarted>,
    mut commands: Commands,
    lobby: Res<Lobby>,
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
                PlayerId(id),
                Position::from(Vec2::new(-50.0, (f32::from(slot) - center) * spacing)),
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
        if Some(*peer) == local {
            commands.entity(player).insert(player_input_map());
        }
    }
}
