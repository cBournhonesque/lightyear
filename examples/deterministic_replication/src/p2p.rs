//! Direct P2P setup for the deterministic Avian simulation.
//!
//! Every peer creates the same fixed physics world and player roster locally. No entity state is
//! replicated and no peer is authoritative; only tick-indexed player inputs cross the network.

use crate::protocol::{PlayerActions, PlayerActivationTick, PlayerId};
use crate::shared;
use bevy::prelude::*;
use leafwing_input_manager::prelude::ActionState;
use lightyear::input::leafwing::prelude::LeafwingBuffer;
use lightyear::prediction::rollback::DeterministicPredicted;
use lightyear::prelude::input::client::InputSystems;
use lightyear::prelude::*;
use lightyear_deterministic_replication::prelude::CatchUpMode;
use lightyear_examples_common::p2p::{
    GAMEPLAY_START_TICK, P2PGameplayStarted, P2PSettings, input_target_for_peer,
};

/// Namespace for stable deterministic-replication player hashes on the input wire.
const PLAYER_INPUT_HASH_BASE: u64 = 0x4445_5445_524D_0000;

pub struct ExampleP2PPlugin;

impl Plugin for ExampleP2PPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            FixedPreUpdate,
            spawn_fixed_world.before(InputSystems::BufferClientInputs),
        );
    }
}

/// Start the complete deterministic world in stable order once timeline synchronization finishes.
fn spawn_fixed_world(
    mut commands: Commands,
    _synced: SyncedInputTimeline,
    timeline: Res<LocalTimeline>,
    mode: Res<CatchUpMode>,
    started: Option<Res<P2PGameplayStarted>>,
    settings: Res<P2PSettings>,
    links: Query<(Entity, &RemoteId), With<P2P>>,
) {
    if started.is_some() || timeline.tick().0 < GAMEPLAY_START_TICK {
        return;
    }
    commands.insert_resource(P2PGameplayStarted);

    // P2P has no authoritative state source, so every peer starts from the same input-only world.
    debug_assert_eq!(*mode, CatchUpMode::InputOnly);
    shared::spawn_world(&mut commands, &mode, false, true);

    for peer_id in settings.peer_ids() {
        let id = PeerId::Entity(u64::from(peer_id));
        let input_target = input_target_for_peer(
            &settings,
            &links,
            peer_id,
            PLAYER_INPUT_HASH_BASE | u64::from(peer_id),
        );
        commands.spawn((
            PlayerId(id),
            PlayerActivationTick(Tick(GAMEPLAY_START_TICK)),
            shared::player_bundle(id),
            DeterministicPredicted {
                skip_despawn: true,
                enable_rollback_after: 0,
            },
            input_target,
            ActionState::<PlayerActions>::default(),
            LeafwingBuffer::<PlayerActions>::default(),
        ));
    }
}
