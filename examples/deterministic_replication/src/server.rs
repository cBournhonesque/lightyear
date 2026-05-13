use crate::automation::AutomationServerPlugin;
use crate::protocol::*;
use crate::shared::player_bundle;
use avian2d::prelude::*;
use bevy::prelude::*;
use lightyear::input::leafwing::prelude::LeafwingBuffer;
use lightyear::prediction::rollback::DeterministicPredicted;
use lightyear::prelude::server::input::InputSystems as ServerInputSystems;
use lightyear::prelude::server::*;
use lightyear::prelude::*;
use lightyear_deterministic_replication::prelude::{
    AppCatchUpExt, CatchUpGated, CatchUpMode, CatchUpServerReadiness, CatchUpSystems,
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
        app.add_observer(handle_disconnected);
        app.add_systems(
            PreUpdate,
            update_player_activation_ticks
                .before(update_catch_up_server_readiness)
                .before(CatchUpSystems::UpdateReadiness),
        );
        app.add_systems(
            PreUpdate,
            update_catch_up_server_readiness.in_set(CatchUpSystems::UpdateReadiness),
        );
        app.add_systems(
            FixedPreUpdate,
            assert_active_player_inputs_are_available.after(ServerInputSystems::UpdateActionState),
        );
    }
}

fn assert_active_player_inputs_are_available(
    timeline: Res<LocalTimeline>,
    players: Query<(
        Entity,
        &PlayerId,
        &PlayerActivationTick,
        Option<&LeafwingBuffer<PlayerActions>>,
    )>,
) {
    let current_tick = timeline.tick();
    for (entity, player_id, activation_tick, buffer) in &players {
        if activation_tick.is_pending() || current_tick < activation_tick.0 {
            continue;
        }
        let (last_remote_tick, buffer_end_tick, exact_input_available) = match buffer {
            Some(buffer) => (
                buffer.last_remote_tick,
                buffer.end_tick(),
                buffer.get(current_tick).is_some(),
            ),
            None => (None, None, false),
        };
        let covered =
            matches!(last_remote_tick, Some(tick) if tick >= current_tick) && exact_input_available;
        if !covered {
            error!(
                ?entity,
                player_id = ?player_id.0,
                ?current_tick,
                ?last_remote_tick,
                ?buffer_end_tick,
                exact_input_available,
                "deterministic server is missing real input before simulating an active player tick"
            );
        }
        assert!(
            covered,
            "deterministic server missing real input for player {:?} at tick {:?}; \
             last_remote_tick={:?}, buffer_end_tick={:?}, exact_input_available={}",
            player_id.0, current_tick, last_remote_tick, buffer_end_tick, exact_input_available
        );
    }
}

fn update_player_activation_ticks(
    timeline: Res<LocalTimeline>,
    mut players: Query<(
        &PlayerId,
        &mut PlayerActivationTick,
        Option<&LeafwingBuffer<PlayerActions>>,
    )>,
) {
    let current_tick = timeline.tick();
    for (player_id, mut activation_tick, buffer) in &mut players {
        if !activation_tick.is_pending() {
            continue;
        }
        let Some(buffer) = buffer else {
            continue;
        };
        if !matches!(buffer.last_remote_tick, Some(tick) if tick >= current_tick) {
            continue;
        }
        activation_tick.0 = current_tick + PlayerActivationTick::DELAY_TICKS as i32;
        info!(
            player_id = ?player_id.0,
            ?current_tick,
            activation_tick = ?activation_tick.0,
            "Activating deterministic player after input rebroadcast warmup"
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
/// Checking only `start_tick` is not sufficient: that proves a client sent
/// at least one input packet, but not that the server has real inputs for
/// the tick it is about to snapshot. A catch-up snapshot emitted while the
/// server is still extrapolating a client's input will diverge when the
/// joining client rolls forward with the real rebroadcast input stream.
fn update_catch_up_server_readiness(
    timeline: Res<LocalTimeline>,
    players: Query<
        (
            Option<&LeafwingBuffer<PlayerActions>>,
            &PlayerActivationTick,
        ),
        With<PlayerId>,
    >,
    mut readiness: ResMut<CatchUpServerReadiness>,
) {
    let current_tick = timeline.tick();
    let mut any = false;
    let ready = players.iter().all(|(buffer, activation_tick)| {
        any = true;
        if activation_tick.is_pending() {
            return false;
        }
        match buffer {
            Some(b) => matches!(b.last_remote_tick, Some(t) if t >= current_tick),
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
    players: Query<(Entity, &PlayerId)>,
    mode: Res<CatchUpMode>,
    mut commands: Commands,
) {
    let Ok(remote_id) = query.get(trigger.entity) else {
        return;
    };
    despawn_players_for_remote(
        *remote_id,
        "before reconnect spawn",
        &players,
        &mut commands,
    );
    info!("Spawning player entity for client {:?}", remote_id);
    let mut player = commands.spawn((
        Replicate::to_clients(NetworkTarget::All),
        PlayerId(remote_id.0),
        PlayerActivationTick::pending(),
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
    ));
    if *mode == CatchUpMode::StateBasedCatchUp {
        // `CatchUpGated` hides the registered physics components
        // (Position, Rotation, LinearVelocity, AngularVelocity) from
        // every client. Each client sends a bodyless `CatchUpRequest`
        // when it has a pending catch-up marker, and the server flips
        // visibility to *visible* for every `CatchUpGated` entity at once
        // — producing a single coherent bundled snapshot.
        // Structural components (`PlayerId`, etc.) still replicate
        // immediately, so clients see the entity + marker and can
        // subscribe to rebroadcast inputs right away.
        player.insert(CatchUpGated);
    }
}

/// Remove the simulation entity owned by a disconnecting client.
///
/// The deterministic example treats player entities as connection-bound.
/// Leaving the old player alive after a disconnect means that reconnecting
/// with the same [`RemoteId`] creates duplicate `PlayerId(remote)` entities;
/// the reconnecting client then assigns local input to both, and the next
/// catch-up snapshot diverges immediately.
pub(crate) fn handle_disconnected(
    trigger: On<Add, Disconnected>,
    query: Query<&RemoteId, With<ClientOf>>,
    players: Query<(Entity, &PlayerId)>,
    mut commands: Commands,
) {
    let Ok(remote_id) = query.get(trigger.entity) else {
        return;
    };
    despawn_players_for_remote(*remote_id, "on disconnect", &players, &mut commands);
}

fn despawn_players_for_remote(
    remote_id: RemoteId,
    reason: &'static str,
    players: &Query<(Entity, &PlayerId)>,
    commands: &mut Commands,
) {
    for (entity, player_id) in players
        .iter()
        .filter(|(_, player_id)| player_id.0 == remote_id.0)
    {
        info!(
            ?entity,
            ?remote_id,
            reason,
            "Despawning deterministic player entity"
        );
        commands.entity(entity).despawn();
    }
}
