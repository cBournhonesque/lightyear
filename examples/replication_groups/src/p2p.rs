//! Input-only P2P setup for the replication-groups example.
//!
//! Replication groups remain demonstrated by the conventional server mode. In P2P mode every peer
//! creates the same snake roster locally and exchanges only deterministic inputs.

extern crate alloc;

use crate::protocol::*;
use alloc::collections::VecDeque;
use bevy::prelude::*;
use lightyear::p2p::Lobby;
use lightyear::prediction::rollback::DeterministicPredicted;
use lightyear::prelude::input::native::{ActionState, InputMarker};
use lightyear::prelude::input::InputBuffer;
use lightyear::prelude::*;
use lightyear_deterministic_replication::prelude::DeterministicReplicationPlugin;
use lightyear_examples_common::p2p::input_target_for_peer;
use lightyear_frame_interpolation::FrameInterpolate;
const PLAYER_INPUT_HASH_BASE: u64 = 0x4752_4F55_5000_0000;

pub struct ExampleP2PPlugin;

impl Plugin for ExampleP2PPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(PredictionManager::default());
        app.add_plugins(DeterministicReplicationPlugin);
        app.add_observer(spawn_fixed_roster);
    }
}

fn spawn_fixed_roster(
    _trigger: On<P2PStarted>,
    mut commands: Commands,
    lobby: Res<Lobby>,
    links: Query<(Entity, &RemoteId), With<P2P>>,
) {
    let spacing = 180.0;
    let roster = lobby.roster();
    let local = lobby.local();
    let center = (roster.len() as f32 - 1.0) * 0.5;
    for (slot, peer) in roster.iter().enumerate() {
        let slot = u8::try_from(slot).expect("P2P roster slot fits in u8");
        let id = PeerId::Entity(u64::from(slot));
        let position = Vec2::new((f32::from(slot) - center) * spacing, 0.0);
        let color = Color::hsl((f32::from(slot) * 0.23) % 1.0, 0.8, 0.5);
        let target = input_target_for_peer(
            &lobby,
            &links,
            *peer,
            PLAYER_INPUT_HASH_BASE | u64::from(slot),
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
        if Some(*peer) == local {
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
