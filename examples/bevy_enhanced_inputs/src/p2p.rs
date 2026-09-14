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
use lightyear::p2p::Lobby;
use lightyear::prediction::rollback::DeterministicPredicted;
use lightyear::prelude::*;
use lightyear_deterministic_replication::prelude::DeterministicReplicationPlugin;
use lightyear_examples_common::p2p::input_target_for_peer;

/// Namespace for stable BEI action hashes on the input wire.
const MOVEMENT_INPUT_HASH_BASE: u64 = 0x4245_495F_4D4F_0000;

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
    let roster = lobby.roster();
    let local = lobby.local();
    for (slot, peer) in roster.iter().enumerate() {
        let slot = u8::try_from(slot).expect("P2P roster slot fits in u8");
        let id = PeerId::Entity(u64::from(slot));
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
                Name::new(format!("P2P Player {slot}")),
            ))
            .id();
        if Some(*peer) == local {
            commands.entity(player).insert(Controlled);
        }

        let input_target = input_target_for_peer(
            &lobby,
            &links,
            *peer,
            MOVEMENT_INPUT_HASH_BASE | u64::from(slot),
        );
        let action = commands
            .spawn((
                ActionOf::<Player>::new(player),
                Action::<Movement>::new(),
                BEIBuffer::<Player>::default(),
                input_target,
                Name::new(format!("P2P Movement {slot}")),
            ))
            .id();
        if Some(*peer) == local {
            commands.entity(action).insert((
                Bindings::spawn(Cardinal::wasd_keys()),
                InputMarker::<Player>::default(),
            ));
        } else {
            commands.entity(action).insert(ExternallyMocked);
        }
    }
}
