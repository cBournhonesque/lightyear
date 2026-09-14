//! Direct P2P setup for the simple-box deterministic simulation.
//!
//! Unlike the conventional mode, no server spawns or replicates players. Every peer creates the
//! same fixed roster locally and only exchanges tick-indexed inputs.

use crate::protocol::{Inputs, PlayerBundle, PlayerId, PlayerPosition};
use bevy::prelude::*;
use lightyear::prediction::rollback::DeterministicPredicted;
use lightyear::prelude::input::native::NativeStateSequence;
use lightyear::prelude::input::native::{ActionState, InputMarker};
use lightyear::prelude::input::InputBuffer;
use lightyear::prelude::*;
use lightyear_deterministic_replication::join_catch_up::P2PCatchUpReplay;
use lightyear_deterministic_replication::prelude::JoinCatchUpPlugin;
use lightyear_examples_common::p2p::{input_target_for_peer, P2PSettings};

/// Namespace for stable simple-box player hashes on the input wire.
const PLAYER_INPUT_HASH_BASE: u64 = 0x5349_4D50_4C45_0000;

/// Hash a player's simulated position for the determinism checksum.
///
/// The checksum has to be identical on every peer, so this hashes the float's bit pattern rather
/// than going through any comparison or formatting.
fn hash_player_position(position: &PlayerPosition, hasher: &mut seahash::SeaHasher) {
    use core::hash::Hasher;
    hasher.write_u64(u64::from(position.0.x.to_bits()));
    hasher.write_u64(u64::from(position.0.y.to_bits()));
}

pub struct ExampleP2PPlugin;

impl Plugin for ExampleP2PPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(PredictionManager::default());
        // Register the state this example simulates for the determinism checksum.
        //
        // Without a hash function for it, every checksum is zero, and comparing two of them agrees
        // trivially — so the catch-up's check that its replay matches the session it replayed from
        // would pass without looking at anything.
        app.component::<PlayerPosition>()
            .add_custom_hash(hash_player_position);
        app.add_plugins(lightyear_deterministic_replication::prelude::ChecksumPlugin);
        // A peer that joins has to be caught up with the session it is joining. The session is
        // input-only and every peer already simulates every player's inputs, so replaying them
        // re-derives the world without anyone having to act as an authority.
        app.add_plugins(JoinCatchUpPlugin::<NativeStateSequence<Inputs>>::enabled());
        app.add_observer(spawn_fixed_roster);
        app.add_observer(spawn_initial_world_as_joiner);
        app.add_observer(rebuild_initial_world_as_joiner);
        // A peer that joins a running session is not in its initial roster, so its player is built
        // when it becomes one rather than at session start.
        app.add_observer(spawn_joined_player);
        crate::automation::add_p2p_debugging(app);
    }
}

/// Create incumbent input targets before the transfer so live candidate inputs are buffered.
fn spawn_initial_world_as_joiner(
    trigger: On<P2PJoinCatchUp>,
    mut commands: Commands,
    session: Res<P2PSession>,
    settings: Res<P2PSettings>,
    links: Query<(Entity, &RemoteId), With<P2P>>,
    existing: Query<Entity, With<PlayerId>>,
) {
    info!(
        start_tick = ?trigger.session_start_tick,
        "building input receive targets as a joiner"
    );
    // A retry replaces any world left by an earlier attempt.
    for entity in &existing {
        commands.entity(entity).despawn();
    }

    let started = session.started_peers();
    spawn_roster(
        &mut commands,
        &settings,
        &links,
        settings
            .peer_ids()
            .filter(|peer| started.contains(&PeerId::Entity(u64::from(*peer)))),
    );
}

/// Reset the world before replay; the catch-up plugin preserves buffered inputs by stable hash.
fn rebuild_initial_world_as_joiner(
    trigger: On<P2PCatchUpReplay>,
    mut commands: Commands,
    settings: Res<P2PSettings>,
    links: Query<(Entity, &RemoteId), With<P2P>>,
    existing: Query<Entity, With<PlayerId>>,
) {
    info!(
        start_tick = ?trigger.session_start_tick,
        target_tick = ?trigger.target_tick,
        "rebuilding the initial session world for replay"
    );
    for entity in &existing {
        commands.entity(entity).despawn();
    }

    spawn_roster(
        &mut commands,
        &settings,
        &links,
        settings
            .founding_peer_ids()
            .filter(|peer| *peer != settings.local_peer_id),
    );
}

/// Spawn the founding cohort, including the local player.
fn spawn_fixed_roster(
    _trigger: On<P2PStarted>,
    mut commands: Commands,
    settings: Res<P2PSettings>,
    links: Query<(Entity, &RemoteId), With<P2P>>,
) {
    spawn_roster(
        &mut commands,
        &settings,
        &links,
        settings.founding_peer_ids(),
    );
}

/// Keep roster order and input identities identical across founding and catch-up.
///
/// A joiner's local player must remain absent until [`P2PJoined`].
fn spawn_roster(
    commands: &mut Commands,
    settings: &P2PSettings,
    links: &Query<(Entity, &RemoteId), With<P2P>>,
    peers: impl Iterator<Item = u8>,
) {
    for peer_id in peers {
        let hash = PLAYER_INPUT_HASH_BASE | u64::from(peer_id);
        let target = input_target_for_peer(settings, links, peer_id, hash);
        let is_local = peer_id == settings.local_peer_id;
        spawn_player(
            commands,
            settings,
            PeerId::Entity(u64::from(peer_id)),
            u64::from(peer_id),
            target,
            is_local,
        );
    }
}

/// Build the player of a peer that joined the running session, when it becomes one.
///
/// The tick matters as much as the entity: the player is created on the tick it is first simulated
/// at, so it never exists on one peer and not another. Creating it any earlier would also make this
/// peer report the newcomer as a remote input stream it is still waiting on.
fn spawn_joined_player(trigger: On<P2PJoined>, mut commands: Commands, settings: Res<P2PSettings>) {
    // A peer that is not the newcomer knows which Link leads to it, and scoping the target to that
    // Link is what routes the newcomer's inputs onto this player rather than onto a like-named one.
    // The newcomer itself originates its own input, so its target is unscoped.
    let mut target = PreSpawned::new(PLAYER_INPUT_HASH_BASE | trigger.peer_id.to_bits());
    if let Some(link) = trigger.link {
        target = target.for_receiver(link);
    }

    info!(
        peer = ?trigger.peer_id,
        activate_tick = ?trigger.activate_tick,
        local_is_joiner = trigger.local_is_joiner,
        "a peer joined the session; spawning its player"
    );
    spawn_player(
        &mut commands,
        &settings,
        trigger.peer_id,
        trigger.peer_id.to_bits(),
        target,
        trigger.local_is_joiner,
    );
}

/// Spawn one deterministic player.
///
/// `roster_index` places it on the same spot on every peer; the examples encode a peer's roster
/// index in its id, so a peer that joined beyond the original roster still gets a stable slot.
fn spawn_player(
    commands: &mut Commands,
    settings: &P2PSettings,
    peer_id: PeerId,
    roster_index: u64,
    target: PreSpawned,
    is_local: bool,
) {
    let spacing = 120.0;
    let center = (f32::from(settings.player_count) - 1.0) * 0.5;
    let position = Vec2::new((roster_index as f32 - center) * spacing, 0.0);

    let entity = commands
        .spawn((
            PlayerBundle::new(peer_id, position),
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
        // Only the local player captures input; every other player's input arrives over its Link.
        commands
            .entity(entity)
            .insert(InputMarker::<Inputs>::default());
    }
}
