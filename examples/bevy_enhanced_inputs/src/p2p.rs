//! Direct P2P setup for the Bevy Enhanced Input example.
//!
//! Player contexts and their action entities are created locally in the same stable roster order
//! on every peer. The action entity carries the stable input-wire identity because BEI input
//! messages target actions rather than their player context.

use crate::protocol::{Movement, Player, PlayerColor, PlayerId, PlayerPosition};
use crate::shared;
use bevy::prelude::*;
use bevy_enhanced_input::context::ExternallyMocked;
use lightyear::input::bei::prelude::{
    Action, ActionOf, BEIBuffer, Bindings, Cardinal, InputMarker,
};
use lightyear::prediction::rollback::DeterministicPredicted;
use lightyear::prelude::input::client::InputSystems;
use lightyear::prelude::*;
use lightyear_deterministic_replication::prelude::DeterministicReplicationPlugin;
use lightyear_examples_common::p2p::{
    input_target_for_peer, P2PGameplayStarted, P2PSettings, GAMEPLAY_START_TICK,
};

/// Namespace for stable BEI action hashes on the input wire.
const MOVEMENT_INPUT_HASH_BASE: u64 = 0x4245_495F_4D4F_0000;

pub struct ExampleP2PPlugin;

impl Plugin for ExampleP2PPlugin {
    fn build(&self, app: &mut App) {
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
    _synced: SyncedInputTimeline,
    timeline: Res<LocalTimeline>,
    started: Option<Res<P2PGameplayStarted>>,
    settings: Res<P2PSettings>,
    links: Query<(Entity, &RemoteId), With<P2P>>,
) {
    if started.is_some() || timeline.tick().0 < GAMEPLAY_START_TICK {
        return;
    }
    commands.insert_resource(P2PGameplayStarted);

    for peer_id in settings.peer_ids() {
        let id = PeerId::Entity(u64::from(peer_id));
        let player = commands
            .spawn((
                Player,
                PlayerId(id),
                PlayerPosition(shared::initial_player_position(id)),
                PlayerColor(shared::color_from_id(id)),
                DeterministicPredicted {
                    skip_despawn: true,
                    enable_rollback_after: 0,
                },
                Name::new(format!("P2P Player {peer_id}")),
            ))
            .id();
        if peer_id == settings.local_peer_id {
            commands.entity(player).insert(Controlled);
        }

        let input_target = input_target_for_peer(
            &settings,
            &links,
            peer_id,
            MOVEMENT_INPUT_HASH_BASE | u64::from(peer_id),
        );
        let action = commands
            .spawn((
                ActionOf::<Player>::new(player),
                Action::<Movement>::new(),
                BEIBuffer::<Player>::default(),
                input_target,
                Name::new(format!("P2P Movement {peer_id}")),
            ))
            .id();
        if peer_id == settings.local_peer_id {
            commands.entity(action).insert((
                Bindings::spawn(Cardinal::wasd_keys()),
                InputMarker::<Player>::default(),
            ));
        } else {
            commands.entity(action).insert(ExternallyMocked);
        }
    }
}
