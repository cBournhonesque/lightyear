//! Direct P2P setup for the simple-box deterministic simulation.
//!
//! Unlike the conventional mode, no server spawns or replicates players. Every peer creates the
//! same fixed roster locally and only exchanges tick-indexed inputs.

use crate::protocol::{Inputs, PlayerBundle};
use bevy::prelude::*;
use lightyear::p2p::Lobby;
use lightyear::prediction::rollback::DeterministicPredicted;
use lightyear::prelude::input::native::{ActionState, InputMarker};
use lightyear::prelude::input::InputBuffer;
use lightyear::prelude::*;
use lightyear_examples_common::p2p::input_target_for_peer;

/// Namespace for stable simple-box player hashes on the input wire.
const PLAYER_INPUT_HASH_BASE: u64 = 0x5349_4D50_4C45_0000;

pub struct ExampleP2PPlugin;

impl Plugin for ExampleP2PPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(PredictionManager::default());
        app.add_plugins(
            lightyear_deterministic_replication::prelude::DeterministicReplicationPlugin,
        );
        app.add_observer(spawn_fixed_roster);
        crate::automation::add_p2p_debugging(app);
    }
}

/// Spawn one local deterministic copy of every player in stable roster order.
///
/// The local player receives [`InputMarker`], while remote players start with empty input buffers
/// so prediction can begin before their first packet arrives. The explicit [`PreSpawned`] hash is
/// the cross-world input identity; P2P deliberately does not depend on replication entity maps.
fn spawn_fixed_roster(
    _trigger: On<P2PStarted>,
    mut commands: Commands,
    lobby: Res<Lobby>,
    links: Query<(Entity, &RemoteId), With<P2P>>,
) {
    let spacing = 120.0;
    let roster = lobby.roster();
    let local = lobby.local();
    let center = (roster.len() as f32 - 1.0) * 0.5;

    for (slot, peer) in roster.iter().enumerate() {
        let slot = u8::try_from(slot).expect("P2P roster slot fits in u8");
        let id = PeerId::Entity(u64::from(slot));
        let position = Vec2::new((f32::from(slot) - center) * spacing, 0.0);
        let hash = PLAYER_INPUT_HASH_BASE | u64::from(slot);
        let pre_spawned = input_target_for_peer(&lobby, &links, *peer, hash);

        let entity = commands
            .spawn((
                PlayerBundle::new(id, position),
                DeterministicPredicted {
                    skip_despawn: true,
                    enable_rollback_after: 0,
                },
                pre_spawned,
                ActionState::<Inputs>::default(),
                InputBuffer::<ActionState<Inputs>, Inputs>::default(),
            ))
            .id();
        if Some(*peer) == local {
            commands
                .entity(entity)
                .insert(InputMarker::<Inputs>::default());
        }
    }
}
