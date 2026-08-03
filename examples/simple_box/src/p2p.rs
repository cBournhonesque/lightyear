//! Direct P2P setup for the simple-box deterministic simulation.
//!
//! Unlike the conventional mode, no server spawns or replicates players. Every peer creates the
//! same fixed roster locally and only exchanges tick-indexed inputs.

use crate::protocol::{Inputs, PlayerBundle};
use bevy::prelude::*;
use lightyear::prediction::rollback::DeterministicPredicted;
use lightyear::prelude::input::native::{ActionState, InputMarker};
use lightyear::prelude::input::InputBuffer;
use lightyear::prelude::*;
use lightyear_examples_common::p2p::P2PSettings;

/// Namespace for stable simple-box player hashes on the input wire.
const PLAYER_INPUT_HASH_BASE: u64 = 0x5349_4D50_4C45_0000;

/// Tick at which the raw fixed-roster example begins applying buffered inputs.
///
/// This gives every peer time to complete initial timeline synchronization and exchange its first
/// input window. The future session handshake will replace this local example convention with an
/// agreed start epoch.
pub(crate) const GAMEPLAY_START_TICK: u32 = 120;

pub struct ExampleP2PPlugin;

impl Plugin for ExampleP2PPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(PredictionManager::default());
        app.add_plugins(
            lightyear_deterministic_replication::prelude::DeterministicReplicationPlugin,
        );
        app.add_systems(Startup, spawn_fixed_roster);
        crate::automation::add_p2p_debugging(app);
    }
}

/// Spawn one local deterministic copy of every player in stable roster order.
///
/// The local player receives [`InputMarker`], while remote players start with empty input buffers
/// so prediction can begin before their first packet arrives. The explicit [`PreSpawned`] hash is
/// the cross-world input identity; P2P deliberately does not depend on replication entity maps.
fn spawn_fixed_roster(
    mut commands: Commands,
    settings: Res<P2PSettings>,
    links: Query<(Entity, &RemoteId), With<P2P>>,
) {
    let spacing = 120.0;
    let center = (f32::from(settings.player_count) - 1.0) * 0.5;

    for peer_id in settings.peer_ids() {
        let id = PeerId::Entity(u64::from(peer_id));
        let position = Vec2::new((f32::from(peer_id) - center) * spacing, 0.0);
        let hash = PLAYER_INPUT_HASH_BASE | u64::from(peer_id);
        let mut pre_spawned = PreSpawned::new(hash);
        if peer_id != settings.local_peer_id {
            let owner_link = links
                .iter()
                .find_map(|(entity, remote_id)| (remote_id.0 == id).then_some(entity))
                .unwrap_or_else(|| panic!("missing P2P Link for roster peer {peer_id}"));
            pre_spawned = pre_spawned.for_receiver(owner_link);
        }

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
        if peer_id == settings.local_peer_id {
            commands
                .entity(entity)
                .insert(InputMarker::<Inputs>::default());
        }
    }
}
