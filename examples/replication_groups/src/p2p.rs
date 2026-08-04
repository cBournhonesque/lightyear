//! Input-only P2P setup for the replication-groups example.
//!
//! Replication groups remain demonstrated by the conventional server mode. In P2P mode every peer
//! creates the same snake roster locally and exchanges only deterministic inputs.

extern crate alloc;

use crate::protocol::*;
use alloc::collections::VecDeque;
use bevy::prelude::*;
use lightyear::prediction::rollback::DeterministicPredicted;
use lightyear::prelude::input::client::InputSystems;
use lightyear::prelude::input::native::{ActionState, InputMarker};
use lightyear::prelude::input::InputBuffer;
use lightyear::prelude::*;
use lightyear_deterministic_replication::prelude::DeterministicReplicationPlugin;
use lightyear_examples_common::p2p::{
    input_target_for_peer, insert_example_session, P2PGameplayStarted, P2PSettings,
};
use lightyear_frame_interpolation::FrameInterpolate;
const PLAYER_INPUT_HASH_BASE: u64 = 0x4752_4F55_5000_0000;

pub struct ExampleP2PPlugin;

impl Plugin for ExampleP2PPlugin {
    fn build(&self, app: &mut App) {
        insert_example_session(app, PLAYER_INPUT_HASH_BASE);
        app.insert_resource(PredictionManager::default());
        app.add_plugins(DeterministicReplicationPlugin);
        app.add_systems(
            FixedPreUpdate,
            spawn_fixed_roster.before(InputSystems::BufferClientInputs),
        );
    }
}

fn spawn_fixed_roster(
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

    let spacing = 180.0;
    let center = (f32::from(settings.player_count) - 1.0) * 0.5;
    for peer_id in settings.peer_ids() {
        let id = PeerId::Entity(u64::from(peer_id));
        let position = Vec2::new((f32::from(peer_id) - center) * spacing, 0.0);
        let color = Color::hsl((f32::from(peer_id) * 0.23) % 1.0, 0.8, 0.5);
        let target = input_target_for_peer(
            &settings,
            &links,
            peer_id,
            PLAYER_INPUT_HASH_BASE | u64::from(peer_id),
        );
        let player = commands
            .spawn((
                PlayerId(id),
                PlayerPosition(position),
                PlayerColor(color),
                DeterministicPredicted {
                    skip_despawn: true,
                    enable_rollback_after: 0,
                },
                target,
                ActionState::<Inputs>::default(),
                InputBuffer::<ActionState<Inputs>, Inputs>::default(),
                FrameInterpolate,
                Name::from("P2P Head"),
            ))
            .id();
        if peer_id == settings.local_peer_id {
            commands
                .entity(player)
                .insert(InputMarker::<Inputs>::default());
        }

        let tail_length = 300.0;
        let direction = Direction::Up;
        let mut points = VecDeque::new();
        points.push_front((direction.get_tail(position, tail_length), direction));
        commands.spawn((
            PlayerParent(player),
            TailPoints(points),
            TailLength(tail_length),
            DeterministicPredicted {
                skip_despawn: true,
                enable_rollback_after: 0,
            },
            Name::from("P2P Tail"),
        ));
    }
}
