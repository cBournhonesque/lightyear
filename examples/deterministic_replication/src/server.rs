use crate::automation::AutomationServerPlugin;
use crate::protocol::*;
use crate::shared::player_bundle;
use avian2d::prelude::*;
use bevy::prelude::*;
use lightyear::input::leafwing::prelude::LeafwingBuffer;
use lightyear::prediction::rollback::DeterministicPredicted;
use lightyear::prelude::server::*;
use lightyear::prelude::*;
use lightyear_deterministic_replication::prelude::{
    AppCatchUpExt, CatchUpGated, CatchUpServerReadiness, CatchUpSystems,
};
use lightyear_examples_common::shared::SEND_INTERVAL;

#[derive(Clone)]
pub struct ExampleServerPlugin;

impl Plugin for ExampleServerPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(AutomationServerPlugin);
        if !app
            .is_plugin_added::<lightyear_deterministic_replication::prelude::ChecksumReceivePlugin>(
            )
        {
            app.add_plugins(lightyear_deterministic_replication::prelude::ChecksumReceivePlugin);
        }
        // The LateJoinCatchUpPlugin itself is added by ProtocolPlugin
        // (in SharedPlugin) so it runs before `cli.spawn_connections`.
        // Register which components are catch-up-gated: they are hidden
        // from each client by default and only sent once after the
        // client explicitly requests catch-up (see
        // `lightyear_deterministic_replication::late_join`).
        app.register_catchup_components::<(Position, Rotation, LinearVelocity, AngularVelocity)>();
        app.insert_resource(ReplicationMetadata::new(SEND_INTERVAL));
        app.add_observer(handle_new_client);
        app.add_observer(handle_connected);
        app.add_systems(
            PreUpdate,
            update_catch_up_server_readiness.in_set(CatchUpSystems::UpdateReadiness),
        );
    }
}

/// Update [`CatchUpServerReadiness`] so the late-join catch-up plugin
/// knows when it's safe to send a bundled snapshot.
///
/// The snapshot computed at server tick `T` is only usable by the
/// requesting client if every other client's inputs have been received by
/// the server up through `T`. Otherwise the server simulated `T` using
/// extrapolated/decayed input for lagging clients, and the snapshot diverges
/// from what the clients' own input buffers say.
///
/// Concretely: `all_clients_ready` is true when every player entity on the
/// server has a `LeafwingBuffer<PlayerActions>` whose `last_remote_tick`
/// is `Some(tick)` with `tick >= server_current_tick`.
///
/// For the local player (one client per server), the `LeafwingBuffer`'s
/// `last_remote_tick` is populated as soon as the first input message from
/// that client arrives. So the readiness flag flips to `true` once inputs
/// have started flowing from every connected client — which is exactly
/// when the bundled snapshot becomes consistent.
/// Update [`CatchUpServerReadiness`] so the late-join catch-up plugin knows
/// when it is safe to send a bundled snapshot.
///
/// Readiness criterion: **every player entity has a non-empty `InputBuffer`
/// with `start_tick <= current_tick`.**
///
/// This guarantees that when the bundled snapshot lands on a client and
/// that client rolls back to the snapshot tick `T`, the client's rebroadcast
/// buffer for every remote player has entries covering the replay window
/// `[T+1, current_tick]`. Without this, the client's `get_action_state`
/// would hit its "no buffered entry" branch during replay and decay the
/// current ActionState — which on a recently-received-first-press remote
/// player is "Pressed", diverging from the server's "default Released"
/// interpretation.
///
/// Equivalently: the snapshot cannot be emitted before every client has
/// sent its first input message to the server. Before a client's first
/// message arrives, neither the server nor any other client knows that
/// client's ActionState for those ticks, and their fallbacks disagree.
fn update_catch_up_server_readiness(
    timeline: Res<LocalTimeline>,
    players: Query<Option<&LeafwingBuffer<PlayerActions>>, With<PlayerId>>,
    mut readiness: ResMut<CatchUpServerReadiness>,
) {
    let current_tick = timeline.tick();
    let mut any = false;
    let ready = players.iter().all(|b| {
        any = true;
        match b {
            Some(b) => matches!(b.start_tick, Some(t) if t <= current_tick),
            None => false,
        }
    });
    readiness.all_clients_ready = any && ready;
}

pub(crate) fn handle_new_client(trigger: On<Add, LinkOf>, mut commands: Commands) {
    commands.entity(trigger.entity).insert(ReplicationSender);
}

/// Spawn the player entity with the full deterministic simulation bundle
/// the moment the client connects.
///
/// The server simulates from here. Physics components are hidden from every
/// client by `CatchUpGated` and only replicated once a client explicitly
/// requests a catch-up snapshot (see
/// [`lightyear_deterministic_replication::late_join`]). That gives the
/// connecting client time to accumulate the rebroadcast-input window
/// required to deterministically replay from the snapshot tick to "now".
pub(crate) fn handle_connected(
    trigger: On<Add, Connected>,
    query: Query<&RemoteId, With<ClientOf>>,
    mut commands: Commands,
) {
    let Ok(remote_id) = query.get(trigger.entity) else {
        return;
    };
    info!("Spawning player entity for client {:?}", remote_id);
    commands.spawn((
        Replicate::to_clients(NetworkTarget::All),
        PlayerId(remote_id.0),
        player_bundle(remote_id.0),
        // `skip_despawn: true` — the player is spawned by a
        // non-deterministic event (the user connecting). Rollback must
        // not despawn the entity, and `enable_rollback_after: N`
        // prevents restoring history for ticks before the entity
        // existed on this peer.
        DeterministicPredicted {
            skip_despawn: true,
            ..default()
        },
        // `CatchUpGated` hides the registered physics components
        // (Position, Rotation, LinearVelocity, AngularVelocity) from
        // every client that has not yet caught up. Each client sends a
        // single bodyless `CatchUpRequest` once it's synced, and the
        // server flips visibility to *visible* for every `CatchUpGated`
        // entity at once — producing a single coherent bundled snapshot.
        // Structural components (`PlayerId`, etc.) still replicate
        // immediately, so clients see the entity + marker and can
        // subscribe to rebroadcast inputs right away.
        CatchUpGated,
    ));
}
