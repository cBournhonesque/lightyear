use avian2d::prelude::*;
use bevy::prelude::*;
use leafwing_input_manager::prelude::*;
use lightyear::prediction::rollback::{AwaitingCatchUpSnapshot, DeterministicPredicted};
use lightyear::prelude::client::*;
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
    client: Option<Single<Entity, (With<Client>, Without<InitialCatchUpComplete>)>>,
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

#[derive(Component)]
struct InitialCatchUpComplete;

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
/// already-caught-up client arrive through normal `replicate_once` and do not
/// need another catch-up rollback.
fn activate_physics_when_bundle_lands(
    mut commands: Commands,
    // Players whose catch-up snapshot has just landed (they now have
    // `Position`) but we haven't yet added local physics components.
    pending: Query<
        (Entity, &PlayerId, &Position, Has<AwaitingCatchUpSnapshot>),
        Without<PhysicsActivated>,
    >,
    // Known remote players that are still waiting for the bundled snapshot
    // (they have `PlayerId` from structural replication but no `Position`
    // yet). The `still_pending` guard ensures the forced rollback fires
    // only when the *entire* bundle has arrived.
    still_pending: Query<Entity, (With<PlayerId>, Without<PhysicsActivated>, Without<Position>)>,
    awaiting_snapshots: Query<(Entity, Option<&ConfirmHistory>), With<AwaitingCatchUpSnapshot>>,
    checkpoints: Res<ReplicationCheckpointMap>,
    mode: Res<CatchUpMode>,
    client: Option<Single<Entity, With<Client>>>,
) {
    let mut activated_awaiting_catchup = false;
    for (entity, player_id, _position, awaiting_catchup) in pending.iter() {
        if awaiting_catchup {
            info!(
                "Client: activating physics for player {:?} (catch-up bundle snapshot landed)",
                player_id.0
            );
            activated_awaiting_catchup = true;
        } else {
            info!(
                "Client: activating physics for player {:?} (normal initial replication)",
                player_id.0
            );
        }
        commands.entity(entity).insert((
            PhysicsBundle::player(),
            ColorComponent(color_from_id(player_id.0)),
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
    // Only fire the single forced rollback once ALL gated players we know
    // about have their catch-up components. Replicon emits the bundle in
    // one update at one tick `S`, so in practice every player gets
    // `Position` on the same frame and the rollback fires once.
    if *mode == CatchUpMode::StateBasedCatchUp && still_pending.is_empty() {
        let Some(reference) = catchup_snapshot_reference(&awaiting_snapshots, &checkpoints) else {
            if activated_awaiting_catchup {
                debug!("Client: waiting for the full catch-up snapshot bundle");
            }
            return;
        };
        let Some(client) = client else {
            return;
        };
        let client = client.into_inner();
        commands.queue(move |world: &mut World| {
            if lightyear_deterministic_replication::prelude::request_forced_rollback_to_catch_up_tick(
                world, reference,
            ) && let Ok(mut client) = world.get_entity_mut(client) {
                client.insert(InitialCatchUpComplete);
            }
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
