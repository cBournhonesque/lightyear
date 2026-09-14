//! Direct P2P setup for the deterministic Avian simulation.
//!
//! Every peer creates the same fixed physics world and player roster locally. No entity state is
//! replicated and no peer is authoritative; only tick-indexed player inputs cross the network.
//!
//! # Building the initial world
//!
//! A peer builds the world exactly once, from the same code, whether it started the session or
//! joined one: the world is a pure function of the session's first gameplay tick. The only
//! difference is which event carries that tick.
//!
//! - A founding peer is told by [`P2PStarted`].
//! - A joiner never receives `P2PStarted`, because it was not there when the session started. It is
//!   told by [`P2PJoinCatchUp`] instead, which fires once its Links to the roster are up and
//!   catch-up can begin.
//!
//! Both events hand the application the session's first gameplay tick, so the two paths converge on
//! [`spawn_session_world`]. That matters because the joiner then replays the session from exactly
//! that tick, so a world built at any other tick would diverge from the peers it is joining.

use crate::client::player_input_map;
use crate::protocol::{PlayerActions, PlayerActivationTick, PlayerId};
use crate::shared;
use bevy::prelude::*;
use leafwing_input_manager::prelude::ActionState;
use lightyear::input::leafwing::prelude::{LeafwingBuffer, LeafwingSequence};
use lightyear::prediction::rollback::DeterministicPredicted;
use lightyear::prelude::*;
use lightyear_deterministic_replication::prelude::{
    CatchUpMode, JoinCatchUpConfig, JoinCatchUpPlugin,
};
use lightyear_examples_common::p2p::{P2PSettings, input_target_for_peer};

/// Namespace for stable deterministic-replication player hashes on the input wire.
const PLAYER_INPUT_HASH_BASE: u64 = 0x4445_5445_524D_0000;

pub struct ExampleP2PPlugin;

impl Plugin for ExampleP2PPlugin {
    fn build(&self, app: &mut App) {
        // A joiner has to be caught up with the session it is joining. Building the world is only
        // the first half of that: without an implementation installed behind `P2PJoinCatchUp`, the
        // attempt would build the world and then wait forever, because the join does not time out
        // while catch-up is outstanding.
        //
        // Input replay is the fit here: the session is input-only, every peer already simulates
        // every player's inputs, and replaying them re-derives the world without anyone having to
        // act as an authority.
        app.add_plugins(JoinCatchUpPlugin::<LeafwingSequence<PlayerActions>>::default());
        app.insert_resource(JoinCatchUpConfig {
            // Recording costs one entry per player per tick for the whole session, so it is only
            // worth paying when a join is possible.
            enabled: true,
            ..default()
        });
        app.add_observer(build_world_on_start);
        app.add_observer(build_world_on_join);
        #[cfg(feature = "p2p-replication-catchup")]
        snapshot_donor::build(app);
    }
}

/// Build the world when this peer founds the session.
fn build_world_on_start(
    trigger: On<P2PStarted>,
    commands: Commands,
    mode: Res<CatchUpMode>,
    settings: Res<P2PSettings>,
    links: Query<(Entity, &RemoteId), With<P2P>>,
) {
    spawn_session_world(
        commands,
        &mode,
        &settings,
        &links,
        trigger.start_tick,
        Cause::Started,
    );
}

/// Build the world when this peer joins a session that is already running.
///
/// The session's first gameplay tick comes from the bootstrap peer, because a joiner never ran the
/// start barrier and so has no other way to know where the session began.
fn build_world_on_join(
    trigger: On<P2PJoinCatchUp>,
    mut commands: Commands,
    mode: Res<CatchUpMode>,
    settings: Res<P2PSettings>,
    links: Query<(Entity, &RemoteId), With<P2P>>,
) {
    let Some(start_tick) = trigger.session_start_tick else {
        // Without it the world cannot be placed on the session's timeline, and a replay would run
        // against a world that means something different to every other peer.
        error!("the bootstrap peer did not report the session start tick; cannot build the world");
        commands.trigger(P2PJoinCancel {
            reason: JoinRejectReason::CatchUpFailed,
        });
        return;
    };
    spawn_session_world(
        commands,
        &mode,
        &settings,
        &links,
        start_tick,
        Cause::Joined,
    );
}

/// Whether this peer founded the session or joined it.
///
/// The world is the same either way; only the catch-up that follows differs, and stating the cause
/// keeps that difference at the one place it belongs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Cause {
    Started,
    Joined,
}

/// Create the complete deterministic world in stable order.
///
/// `start_tick` is the session's first gameplay tick, so every peer builds the same world on the
/// same timeline and the joiner's replay lands on it.
fn spawn_session_world(
    mut commands: Commands,
    mode: &CatchUpMode,
    settings: &P2PSettings,
    links: &Query<(Entity, &RemoteId), With<P2P>>,
    start_tick: Tick,
    cause: Cause,
) {
    // P2P has no authoritative state source, so every peer starts from the same input-only world.
    debug_assert_eq!(*mode, CatchUpMode::InputOnly);
    shared::spawn_world(&mut commands, mode, false, true);

    for peer_id in settings.peer_ids() {
        let id = PeerId::Entity(u64::from(peer_id));
        let input_target = input_target_for_peer(
            settings,
            links,
            peer_id,
            PLAYER_INPUT_HASH_BASE | u64::from(peer_id),
        );
        let player = commands
            .spawn((
                PlayerId(id),
                // Every player is active from the session's first tick, on every peer — including a
                // joiner, which replays that tick rather than observing it live.
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
        if peer_id == settings.local_peer_id {
            commands.entity(player).insert(player_input_map());
        }
    }

    match cause {
        Cause::Started => info!(?start_tick, "built the session world as a founder"),
        Cause::Joined => info!(
            ?start_tick,
            "built the session world as a joiner; catch-up replays the session onto it"
        ),
    }
}

/// Serve the session's state to a newcomer, instead of the newcomer replaying its inputs.
///
/// This is the snapshot flavour of catch-up. It is Replicon's client/server machinery, so the peer
/// serving the snapshot has to look like a Replicon server to the Link the newcomer is on.
///
/// P2P has nowhere else to put that: a `Server` entity alongside an active [`P2P`] Link is rejected
/// as a mixed topology, and `Invalid` unsets the P2P input route. So the client markers go on the
/// Link itself, and the application drives Replicon's `ServerState`, which the server backend would
/// otherwise set from `Started`.
#[cfg(feature = "p2p-replication-catchup")]
mod snapshot_donor {
    use bevy::prelude::*;
    use bevy_replicon::prelude::ServerState;
    use lightyear::connection::client_of::ClientOf;
    use lightyear::prelude::*;

    pub fn build(app: &mut App) {
        app.add_systems(Update, serve_joining_peers);
    }

    /// Admit every peer that is joining into Replication, from the peer that will serve it.
    ///
    /// A peer that is `Candidate` on this peer's side has a Link it dialled in order to join and is
    /// not part of the roster yet — which is exactly the window in which it needs a snapshot. It has
    /// to happen before the newcomer asks, because the request is answered with replicated state.
    fn serve_joining_peers(
        session: Res<P2PSession>,
        mut server_state: ResMut<NextState<ServerState>>,
        mut commands: Commands,
        links: Query<(Entity, &P2P), (With<Connected>, Without<ReplicationSender>)>,
    ) {
        // Only a session that is playing has state to hand over.
        if !matches!(session.state(), P2PSessionState::Started { .. }) {
            return;
        }
        let mut serves = false;
        for (entity, state) in &links {
            if *state != P2P::Candidate {
                continue;
            }
            // `ClientOf` is what the replication send path filters on, and `ReplicationSender` is
            // what admits the Link to replicon's visibility bookkeeping.
            commands
                .entity(entity)
                .insert((ClientOf, ReplicationSender));
            serves = true;
        }
        if serves {
            // Replicon only sends while its server state is running. Nothing else sets it here:
            // the server backend drives it from `Started`, which a P2P peer never has.
            NextState::set_if_neq(&mut server_state, ServerState::Running);
        }
    }
}
