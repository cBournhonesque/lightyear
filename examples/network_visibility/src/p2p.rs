//! Input-only P2P setup for the network-visibility example.
//!
//! Interest management remains a server-side replication demonstration in conventional mode. The
//! P2P mode creates the complete small scene on every peer and exercises only deterministic player
//! input exchange.

use bevy::prelude::*;
use lightyear::prediction::rollback::DeterministicPredicted;
use lightyear::prelude::input::native::{ActionState, InputMarker};
use lightyear::prelude::input::InputBuffer;
use lightyear::prelude::*;
use lightyear_deterministic_replication::prelude::DeterministicReplicationPlugin;
use lightyear_examples_common::p2p::{input_target_for_peer, P2PSettings};

use crate::protocol::*;
use crate::shared::color_from_id;

const PLAYER_INPUT_HASH_BASE: u64 = 0x5649_5349_4200_0000;
const GRID_SIZE: f32 = 200.0;
const NUM_CIRCLES: i32 = 1;

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
    settings: Res<P2PSettings>,
    links: Query<(Entity, &RemoteId), With<P2P>>,
) {
    for x in -NUM_CIRCLES..NUM_CIRCLES {
        for y in -NUM_CIRCLES..NUM_CIRCLES {
            commands.spawn((
                Position(Vec2::new(x as f32 * GRID_SIZE, y as f32 * GRID_SIZE)),
                CircleMarker,
            ));
        }
    }

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
                Position(Vec2::new((f32::from(peer_id) - center) * spacing, 0.0)),
                PlayerColor(color_from_id(id)),
                DeterministicPredicted {
                    skip_despawn: true,
                    enable_rollback_after: 0,
                },
                target,
                ActionState::<Inputs>::default(),
                InputBuffer::<ActionState<Inputs>, Inputs>::default(),
            ))
            .id();
        if peer_id == settings.local_peer_id {
            commands
                .entity(player)
                .insert(InputMarker::<Inputs>::default());
        }
    }
}
