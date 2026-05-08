use avian2d::prelude::*;
use bevy::prelude::*;
use leafwing_input_manager::prelude::*;
use lightyear::prediction::rollback::{AwaitingCatchUpSnapshot, DeterministicPredicted};
use lightyear::prelude::client::*;
use lightyear::prelude::input::leafwing::{LeafwingBuffer, LeafwingSnapshot};
use lightyear::prelude::*;

use crate::automation::AutomationClientPlugin;
use crate::protocol::*;
use crate::shared::color_from_id;
use lightyear_deterministic_replication::prelude::CatchUpMode;

pub struct ExampleClientPlugin;

impl Plugin for ExampleClientPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(AutomationClientPlugin);
        if !app
            .is_plugin_added::<lightyear_deterministic_replication::prelude::ChecksumSendPlugin>()
        {
            app.add_plugins(lightyear_deterministic_replication::prelude::ChecksumSendPlugin);
        }
        // LateJoinCatchUpPlugin itself is added by ProtocolPlugin (in
        // SharedPlugin) so message registration precedes client-entity
        // spawn in `cli.spawn_connections`.
        app.add_systems(
            PreUpdate,
            add_input_map_after_sync.after(ReplicationSystems::Receive),
        );
        app.add_systems(FixedPreUpdate, activate_physics_when_bundle_lands);
        // When a catch-up-gated player replicates to us (structural
        // components arrive first, physics is hidden), mark it
        // `AwaitingCatchUpSnapshot`. This gates `add_confirmed_write` so
        // the eventual Position/Rotation/LinearVelocity/AngularVelocity
        // writes land in `PredictionHistory<C>` (for forced rollback to
        // restore), not on the live component.
        // `request_forced_rollback_to_catch_up_tick` removes the marker
        // once the forced rollback is scheduled.
        app.add_observer(mark_awaiting_catchup_on_replicated_player);
    }
}

fn mark_awaiting_catchup_on_replicated_player(
    trigger: On<Add, PlayerId>,
    // Only replicated-in entities (not server-spawned ones with `Replicate`).
    query: Query<(), (Without<AwaitingCatchUpSnapshot>, Without<Replicate>)>,
    client: Option<Single<(), With<Client>>>,
    mode: Res<CatchUpMode>,
    mut commands: Commands,
) {
    if *mode == CatchUpMode::InputOnly {
        return;
    }
    if client.is_none() {
        return;
    }
    if query.get(trigger.entity).is_ok() {
        commands
            .entity(trigger.entity)
            .insert(AwaitingCatchUpSnapshot);
    }
}

#[derive(Component)]
struct InputMapAdded;

#[derive(Component)]
struct PhysicsActivated;

/// Add an `InputMap` to the local player's replicated entity as soon as the
/// input timeline is synced. This is what lets the local client start
/// sending input messages — without it, the client never broadcasts any
/// input and the server can't rebroadcast it to other peers.
fn add_input_map_after_sync(
    client: Option<Single<&LocalId, (With<Client>, With<IsSynced<InputTimeline>>)>>,
    mut commands: Commands,
    players: Query<(Entity, &PlayerId), (Without<InputMapAdded>, Without<InputMap<PlayerActions>>)>,
) {
    let Some(client) = client else {
        return;
    };
    let local_id = client.into_inner();
    for (entity, player_id) in players.iter() {
        if local_id.0 == player_id.0 {
            info!("Client: adding InputMap to local player {:?}", player_id.0);
            commands.entity(entity).insert((
                InputMap::new([
                    (PlayerActions::Up, KeyCode::KeyW),
                    (PlayerActions::Down, KeyCode::KeyS),
                    (PlayerActions::Left, KeyCode::KeyA),
                    (PlayerActions::Right, KeyCode::KeyD),
                ]),
                InputMapAdded,
            ));
        }
    }
}

/// Activate physics when replicated player state becomes available.
///
/// During the initial state-based catch-up, `AwaitingCatchUpSnapshot` means
/// `add_confirmed_write` has written Position/Rotation/Velocity into
/// `PredictionHistory` at server tick `S`; once the entire bundle lands, we
/// fire a single forced rollback from `S`. Later player spawns on an
/// already-caught-up client use the same path: their physics snapshot is
/// hidden until requested, then the client rolls back from the snapshot tick
/// so it does not miss the spawn-to-receive simulation window.
fn activate_physics_when_bundle_lands(
    mut commands: Commands,
    // Players whose catch-up snapshot has just landed (they now have
    // `Position`) but we haven't yet added local physics components.
    pending: Query<
        (
            Entity,
            &PlayerId,
            &Position,
            Option<&ConfirmHistory>,
            Has<AwaitingCatchUpSnapshot>,
        ),
        Without<PhysicsActivated>,
    >,
    // Known remote players that are still waiting for the bundled snapshot
    // (they have `PlayerId` from structural replication but no `Position`
    // yet). The `still_pending` guard ensures the forced rollback fires
    // only when the *entire* bundle has arrived.
    still_pending: Query<Entity, (With<PlayerId>, Without<PhysicsActivated>, Without<Position>)>,
    awaiting_snapshots: Query<(Entity, Option<&ConfirmHistory>), With<AwaitingCatchUpSnapshot>>,
    mut input_buffers: Query<(&PlayerId, Option<&mut LeafwingBuffer<PlayerActions>>)>,
    checkpoints: Res<ReplicationCheckpointMap>,
    timeline: Res<LocalTimeline>,
    mode: Res<CatchUpMode>,
) {
    let mut activated_awaiting_catchup = false;
    let mut activation_rollback_reference = None;
    let local_tick = timeline.tick();
    let mut ready = Vec::new();
    for (entity, player_id, _position, confirm, awaiting_catchup) in pending.iter() {
        if *mode == CatchUpMode::StateBasedCatchUp {
            let Some(reference_tick) = confirmed_server_tick(confirm, &checkpoints) else {
                debug!(
                    ?entity,
                    player_id = ?player_id.0,
                    "Client: waiting for initial replication checkpoint before activating physics"
                );
                continue;
            };
            if !input_buffers_cover_replay(reference_tick, local_tick, &mut input_buffers) {
                let input_windows = input_buffer_windows(&input_buffers);
                debug!(
                    ?entity,
                    player_id = ?player_id.0,
                    ?reference_tick,
                    ?local_tick,
                    ?input_windows,
                    "Client: waiting for input buffers to cover deterministic activation rollback"
                );
                continue;
            }
        }
        ready.push((entity, player_id.0, awaiting_catchup));
    }
    // Avian's deterministic physics can depend on the order in which bodies
    // are inserted into its internal structures. Late-joining clients can
    // receive replicated players in a different order from the server, so
    // activate every ready player in a stable game-defined order.
    ready.sort_by_key(|(_, player_id, _)| player_id.to_bits());
    for (entity, player_id, awaiting_catchup) in ready {
        if awaiting_catchup {
            info!(
                "Client: activating physics for player {:?} (catch-up bundle snapshot landed)",
                player_id
            );
            activated_awaiting_catchup = true;
        } else {
            info!(
                "Client: activating physics for player {:?} (replicated snapshot landed without awaiting marker)",
                player_id
            );
            if *mode == CatchUpMode::StateBasedCatchUp {
                activation_rollback_reference.get_or_insert(entity);
            }
        }
        commands.entity(entity).insert((
            PhysicsBundle::player(),
            ColorComponent(color_from_id(player_id)),
            Name::from("Player"),
            // `skip_despawn: true` because the player is not spawned
            // deterministically from input. Matches the server.
            DeterministicPredicted {
                skip_despawn: true,
                ..default()
            },
            PhysicsActivated,
        ));
    }
    if let Some(reference) = activation_rollback_reference {
        commands.queue(move |world: &mut World| {
            lightyear_deterministic_replication::prelude::request_forced_rollback_to_catch_up_tick(
                world, reference,
            );
        });
    }
    // Only fire the single forced rollback once ALL gated players we know
    // about have their catch-up components. Replicon emits the bundle in
    // one update at one tick `S`, so in practice every player gets
    // `Position` on the same frame and the rollback fires once.
    if *mode == CatchUpMode::StateBasedCatchUp
        && activated_awaiting_catchup
        && still_pending.is_empty()
    {
        let Some(reference) = catchup_snapshot_reference(&awaiting_snapshots, &checkpoints) else {
            if activated_awaiting_catchup {
                debug!("Client: waiting for the full catch-up snapshot bundle");
            }
            return;
        };
        commands.queue(move |world: &mut World| {
            lightyear_deterministic_replication::prelude::request_forced_rollback_to_catch_up_tick(
                world, reference,
            );
        });
    }
}

fn catchup_snapshot_reference(
    awaiting_snapshots: &Query<(Entity, Option<&ConfirmHistory>), With<AwaitingCatchUpSnapshot>>,
    checkpoints: &ReplicationCheckpointMap,
) -> Option<Entity> {
    let mut reference = None;
    let mut bundled_tick = None;
    for (entity, confirm) in awaiting_snapshots.iter() {
        let confirm = confirm?;
        let tick = confirm.last_tick();
        checkpoints.get(tick)?;
        match bundled_tick {
            Some(expected) if expected != tick => return None,
            Some(_) => {}
            None => {
                bundled_tick = Some(tick);
                reference = Some(entity);
            }
        }
    }
    reference
}

fn confirmed_server_tick(
    confirm: Option<&ConfirmHistory>,
    checkpoints: &ReplicationCheckpointMap,
) -> Option<Tick> {
    confirm
        .map(ConfirmHistory::last_tick)
        .and_then(|tick| checkpoints.get(tick))
}

fn input_buffers_cover_replay(
    reference_tick: Tick,
    local_tick: Tick,
    input_buffers: &mut Query<(&PlayerId, Option<&mut LeafwingBuffer<PlayerActions>>)>,
) -> bool {
    let mut any = false;
    let replay_end_tick = local_tick - 1;
    input_buffers.iter_mut().all(|(player_id, buffer)| {
        any = true;
        let Some(mut buffer) = buffer else {
            return false;
        };
        let Some(end_tick) = buffer.end_tick() else {
            return false;
        };
        if end_tick < replay_end_tick {
            return false;
        }
        if buffer.start_tick.is_none_or(|start| start > reference_tick) {
            let old_start = buffer.start_tick.unwrap_or(reference_tick);
            debug!(
                player_id = ?player_id.0,
                ?reference_tick,
                ?old_start,
                ?end_tick,
                "Padding input buffer prefix before deterministic activation rollback"
            );
            buffer.extend_to_range(reference_tick, end_tick);
            let mut tick = reference_tick;
            while tick < old_start {
                buffer.set(tick, LeafwingSnapshot::default());
                tick = tick + 1;
            }
        }
        true
    }) && any
}

fn input_buffer_windows(
    input_buffers: &Query<(&PlayerId, Option<&mut LeafwingBuffer<PlayerActions>>)>,
) -> Vec<(PeerId, Option<Tick>, Option<Tick>)> {
    input_buffers
        .iter()
        .map(|(player_id, buffer)| {
            (
                player_id.0,
                buffer.and_then(|buffer| buffer.start_tick),
                buffer.and_then(LeafwingBuffer::end_tick),
            )
        })
        .collect()
}
