//! Input-only P2P Avian simulation with lobby discovery and sequential late joins.
//!
//! Founders build the initial world on `P2PStarted`. Joiners first prepare live input targets,
//! then rebuild the founders on `P2PCatchUpReplay` and replay historical `P2PJoined` activations.

use crate::client::player_input_map;
use crate::protocol::{BallMarker, PlayerActions, PlayerActivationTick, PlayerId};
use crate::shared;
use bevy::prelude::*;
use leafwing_input_manager::prelude::ActionState;
use lightyear::input::leafwing::prelude::{LeafwingBuffer, LeafwingSequence};
use lightyear::p2p::Lobby;
use lightyear::prediction::rollback::DeterministicPredicted;
use lightyear::prelude::*;
use lightyear_deterministic_replication::join_catch_up::P2PCatchUpReplay;
use lightyear_deterministic_replication::prelude::{CatchUpMode, JoinCatchUpPlugin};
use lightyear_examples_common::p2p::input_target_for_peer;

const PLAYER_INPUT_HASH_BASE: u64 = 0x4445_5445_524D_0000;

pub struct ExampleP2PPlugin;

impl Plugin for ExampleP2PPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(JoinCatchUpPlugin::<LeafwingSequence<PlayerActions>>::enabled());
        app.add_observer(build_world_on_start);
        app.add_observer(prepare_join_input_targets);
        app.add_observer(rebuild_world_for_replay);
        app.add_observer(spawn_joined_player);
    }
}

fn build_world_on_start(
    trigger: On<P2PStarted>,
    mut commands: Commands,
    mode: Res<CatchUpMode>,
    lobby: Res<Lobby>,
    links: Query<(Entity, &RemoteId), With<P2P>>,
) {
    spawn_session_world(
        &mut commands,
        &mode,
        &lobby,
        &links,
        trigger.start_tick,
        lobby.roster().into_iter(),
    );
}

/// Receive incumbent inputs while the donor transfers history, without simulating a world yet.
fn prepare_join_input_targets(
    _trigger: On<P2PJoinCatchUp>,
    mut commands: Commands,
    session: Res<P2PSession>,
    lobby: Res<Lobby>,
    links: Query<(Entity, &RemoteId), With<P2P>>,
    existing: Query<Entity, Or<(With<PlayerId>, With<BallMarker>, With<shared::Wall>)>>,
) {
    for entity in &existing {
        commands.entity(entity).despawn();
    }
    for peer in session.started_peers() {
        commands.spawn((
            PlayerId(peer),
            input_target_for_peer(
                &lobby,
                &links,
                peer,
                PLAYER_INPUT_HASH_BASE ^ peer.to_bits(),
            ),
            ActionState::<PlayerActions>::default(),
            LeafwingBuffer::<PlayerActions>::default(),
        ));
    }
}

fn rebuild_world_for_replay(
    trigger: On<P2PCatchUpReplay>,
    mut commands: Commands,
    mode: Res<CatchUpMode>,
    lobby: Res<Lobby>,
    links: Query<(Entity, &RemoteId), With<P2P>>,
    existing: Query<Entity, Or<(With<PlayerId>, With<BallMarker>, With<shared::Wall>)>>,
) {
    for entity in &existing {
        commands.entity(entity).despawn();
    }
    spawn_session_world(
        &mut commands,
        &mode,
        &lobby,
        &links,
        trigger.session_start_tick,
        trigger.initial_peers.iter().copied(),
    );
}

fn spawn_session_world(
    commands: &mut Commands,
    mode: &CatchUpMode,
    lobby: &Lobby,
    links: &Query<(Entity, &RemoteId), With<P2P>>,
    start_tick: Tick,
    peers: impl Iterator<Item = PeerId>,
) {
    debug_assert_eq!(*mode, CatchUpMode::InputOnly);
    shared::spawn_world(commands, mode, false, true);
    for (slot, peer) in peers.enumerate() {
        let target =
            input_target_for_peer(lobby, links, peer, PLAYER_INPUT_HASH_BASE ^ peer.to_bits());
        spawn_player(
            commands,
            peer,
            slot,
            start_tick,
            target,
            lobby.local() == Some(peer),
        );
    }
}

/// Slot order is founding order followed by historical activation order, never today's lobby order.
fn spawn_joined_player(
    trigger: On<P2PJoined>,
    mut commands: Commands,
    players: Query<Entity, With<PlayerId>>,
) {
    let mut target = PreSpawned::new(PLAYER_INPUT_HASH_BASE ^ trigger.peer_id.to_bits());
    if let Some(link) = trigger.link {
        target = target.for_receiver(link);
    }
    spawn_player(
        &mut commands,
        trigger.peer_id,
        players.iter().count(),
        trigger.activate_tick,
        target,
        trigger.local_is_joiner,
    );
}

fn spawn_player(
    commands: &mut Commands,
    peer: PeerId,
    slot: usize,
    activate_tick: Tick,
    target: PreSpawned,
    is_local: bool,
) {
    let player = commands
        .spawn((
            PlayerId(peer),
            PlayerActivationTick(activate_tick),
            shared::player_bundle(peer),
            DeterministicPredicted {
                skip_despawn: true,
                enable_rollback_after: 0,
            },
            target,
            ActionState::<PlayerActions>::default(),
            LeafwingBuffer::<PlayerActions>::default(),
        ))
        .insert(avian2d::prelude::Position::from(Vec2::new(
            -50.0,
            slot as f32 * 50.0 - 250.0,
        )))
        .id();
    if is_local {
        commands.entity(player).insert(player_input_map());
    }
}
