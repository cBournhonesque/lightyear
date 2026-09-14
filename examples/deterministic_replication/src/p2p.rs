//! Direct P2P setup for the deterministic Avian simulation.
//!
//! Every peer creates the same fixed physics world and player roster locally. No entity state is
//! replicated and no peer is authoritative; only tick-indexed player inputs cross the network.

use crate::client::player_input_map;
use crate::protocol::{PlayerActions, PlayerActivationTick, PlayerId};
use crate::shared;
use bevy::prelude::*;
use leafwing_input_manager::prelude::ActionState;
use lightyear::input::leafwing::prelude::LeafwingBuffer;
use lightyear::p2p::Lobby;
use lightyear::prediction::rollback::DeterministicPredicted;
use lightyear::prelude::*;
use lightyear_deterministic_replication::prelude::CatchUpMode;
use lightyear_examples_common::p2p::input_target_for_peer;

/// Namespace for stable deterministic-replication player hashes on the input wire.
const PLAYER_INPUT_HASH_BASE: u64 = 0x4445_5445_524D_0000;

pub struct ExampleP2PPlugin;

impl Plugin for ExampleP2PPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(spawn_fixed_world);
    }
}

/// Start the complete deterministic world in stable order once timeline synchronization finishes.
fn spawn_fixed_world(
    trigger: On<P2PStarted>,
    mut commands: Commands,
    mode: Res<CatchUpMode>,
    lobby: Res<Lobby>,
    links: Query<(Entity, &RemoteId), With<P2P>>,
) {
    let start_tick = trigger.start_tick;

    // P2P has no authoritative state source, so every peer starts from the same input-only world.
    debug_assert_eq!(*mode, CatchUpMode::InputOnly);
    shared::spawn_world(&mut commands, &mode, false, true);

    let roster = lobby.roster();
    let local = lobby.local();
    for (slot, peer) in roster.iter().enumerate() {
        let slot = u8::try_from(slot).expect("P2P roster slot fits in u8");
        let id = PeerId::Entity(u64::from(slot));
        let input_target = input_target_for_peer(
            &lobby,
            &links,
            *peer,
            PLAYER_INPUT_HASH_BASE | u64::from(slot),
        );
        let player = commands
            .spawn((
                PlayerId(id),
                PlayerActivationTick(start_tick),
                shared::player_bundle(id),
                DeterministicPredicted {
                    skip_despawn: true,
                    enable_rollback_after: 0,
                },
                input_target,
                ActionState::<PlayerActions>::default(),
                LeafwingBuffer::<PlayerActions>::default(),
            ))
            .id();
        if Some(*peer) == local {
            commands.entity(player).insert(player_input_map());
        }
    }
}
