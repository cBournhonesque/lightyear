//! Input-only P2P simulation with lobby discovery and sequential late joins.

use crate::protocol::{Inputs, PlayerBundle, PlayerId, PlayerPosition};
use bevy::prelude::*;
use lightyear::p2p::Lobby;
use lightyear::prediction::rollback::DeterministicPredicted;
use lightyear::prelude::input::InputBuffer;
use lightyear::prelude::input::native::{ActionState, InputMarker, NativeStateSequence};
use lightyear::prelude::*;
use lightyear_deterministic_replication::join_catch_up::P2PCatchUpReplay;
use lightyear_deterministic_replication::prelude::JoinCatchUpPlugin;
use lightyear_examples_common::p2p::{P2PSettings, input_target_for_peer};

const PLAYER_INPUT_HASH_BASE: u64 = 0x5349_4D50_4C45_0000;

fn hash_player_position(position: &PlayerPosition, hasher: &mut seahash::SeaHasher) {
    use core::hash::Hasher;
    hasher.write_u64(u64::from(position.0.x.to_bits()));
    hasher.write_u64(u64::from(position.0.y.to_bits()));
}

pub struct ExampleP2PPlugin;

impl Plugin for ExampleP2PPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(PredictionManager::default());
        app.component::<PlayerPosition>()
            .add_custom_hash(hash_player_position);
        app.add_plugins(lightyear_deterministic_replication::prelude::ChecksumPlugin);
        app.add_plugins(JoinCatchUpPlugin::<NativeStateSequence<Inputs>>::enabled());
        app.add_observer(spawn_fixed_roster);
        app.add_observer(spawn_initial_world_as_joiner);
        app.add_observer(rebuild_initial_world_as_joiner);
        app.add_observer(spawn_joined_player);
        crate::automation::add_p2p_debugging(app);
    }
}

/// Prepare all incumbent input targets before live candidate inputs arrive.
fn spawn_initial_world_as_joiner(
    _trigger: On<P2PJoinCatchUp>,
    mut commands: Commands,
    session: Res<P2PSession>,
    lobby: Res<Lobby>,
    settings: Res<P2PSettings>,
    links: Query<(Entity, &RemoteId), With<P2P>>,
    existing: Query<Entity, With<PlayerId>>,
) {
    for entity in &existing {
        commands.entity(entity).despawn();
    }
    spawn_roster(
        &mut commands,
        &lobby,
        &settings,
        &links,
        session.started_peers().into_iter(),
    );
}

/// The catch-up plugin preserves live inputs by hash while we restore the founders.
fn rebuild_initial_world_as_joiner(
    trigger: On<P2PCatchUpReplay>,
    mut commands: Commands,
    lobby: Res<Lobby>,
    settings: Res<P2PSettings>,
    links: Query<(Entity, &RemoteId), With<P2P>>,
    existing: Query<Entity, With<PlayerId>>,
) {
    for entity in &existing {
        commands.entity(entity).despawn();
    }
    spawn_roster(
        &mut commands,
        &lobby,
        &settings,
        &links,
        trigger.initial_peers.iter().copied(),
    );
}

fn spawn_fixed_roster(
    _trigger: On<P2PStarted>,
    mut commands: Commands,
    lobby: Res<Lobby>,
    settings: Res<P2PSettings>,
    links: Query<(Entity, &RemoteId), With<P2P>>,
) {
    spawn_roster(
        &mut commands,
        &lobby,
        &settings,
        &links,
        lobby.roster().into_iter(),
    );
}

fn spawn_roster(
    commands: &mut Commands,
    lobby: &Lobby,
    settings: &P2PSettings,
    links: &Query<(Entity, &RemoteId), With<P2P>>,
    peers: impl Iterator<Item = PeerId>,
) {
    for (slot, peer) in peers.enumerate() {
        let target =
            input_target_for_peer(lobby, links, peer, PLAYER_INPUT_HASH_BASE ^ peer.to_bits());
        spawn_player(
            commands,
            settings,
            peer,
            slot,
            target,
            lobby.local() == Some(peer),
        );
    }
}

/// Append in activation order, including historical joins replayed by catch-up.
fn spawn_joined_player(
    trigger: On<P2PJoined>,
    mut commands: Commands,
    settings: Res<P2PSettings>,
    players: Query<Entity, With<PlayerId>>,
) {
    let mut target = PreSpawned::new(PLAYER_INPUT_HASH_BASE ^ trigger.peer_id.to_bits());
    if let Some(link) = trigger.link {
        target = target.for_receiver(link);
    }
    spawn_player(
        &mut commands,
        &settings,
        trigger.peer_id,
        players.iter().count(),
        target,
        trigger.local_is_joiner,
    );
}

fn spawn_player(
    commands: &mut Commands,
    settings: &P2PSettings,
    peer: PeerId,
    slot: usize,
    target: PreSpawned,
    is_local: bool,
) {
    let center = (f32::from(settings.expected_players) - 1.0) * 0.5;
    let position = Vec2::new((slot as f32 - center) * 120.0, 0.0);
    let entity = commands
        .spawn((
            PlayerBundle::new(peer, position),
            DeterministicPredicted {
                skip_despawn: true,
                enable_rollback_after: 0,
            },
            target,
            ActionState::<Inputs>::default(),
            InputBuffer::<ActionState<Inputs>, Inputs>::default(),
        ))
        .id();
    if is_local {
        commands
            .entity(entity)
            .insert(InputMarker::<Inputs>::default());
    }
}
