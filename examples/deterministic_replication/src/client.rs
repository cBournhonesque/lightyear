use avian2d::prelude::*;
use bevy::prelude::*;
use leafwing_input_manager::prelude::*;
use lightyear::input::leafwing::prelude::LeafwingBuffer;
use lightyear::prediction::rollback::DeterministicPredicted;
use lightyear::prelude::client::*;
use lightyear::prelude::*;
use lightyear_deterministic_replication::prelude::{CatchUpReady, PendingCatchUp};

use crate::automation::AutomationClientPlugin;
use crate::protocol::*;
use crate::shared::color_from_id;

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
            (
                add_input_map_after_sync,
                request_catch_up_for_remote_players,
            )
                .after(ReplicationSystems::Receive),
        );
        app.add_systems(Update, mark_catch_up_ready);
        app.add_systems(FixedPreUpdate, activate_physics_at_tick);
    }
}

#[derive(Component)]
struct InputMapAdded;

#[derive(Component)]
struct PhysicsActivated;

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

/// Mark every *remote* player entity that genuinely needs a late-join
/// catch-up snapshot (its `PhysicsStartTick` is already in the past by
/// the time we see it) with `PendingCatchUp`.
///
/// Peers that connect *after* we did — so their `PhysicsStartTick` is in
/// the future when we first see their entity — do NOT need a catch-up
/// snapshot; the deterministic simulation starts at `PhysicsStartTick`
/// identically on every peer, so skipping catch-up avoids sending a
/// useless request and (more importantly) avoids keeping
/// `AwaitingCatchUpSnapshot` on the entity, which would suppress our
/// checksum send for every tick the entity is present.
///
/// We only insert `PendingCatchUp` once per entity: a `CatchUpRequested`
/// marker is added at the same time so subsequent runs skip the entity.
/// Entities without a replicated `PhysicsStartTick` yet are left alone
/// so we revisit them once it arrives.
fn request_catch_up_for_remote_players(
    client: Option<Single<&LocalId, (With<Client>, With<IsSynced<InputTimeline>>)>>,
    timeline: Res<LocalTimeline>,
    mut commands: Commands,
    players: Query<(Entity, &PlayerId, &PhysicsStartTick), Without<CatchUpRequested>>,
) {
    let Some(client) = client else {
        return;
    };
    let local_id = client.into_inner();
    let tick = timeline.tick();
    for (entity, player_id, start) in players.iter() {
        if local_id.0 == player_id.0 {
            // Local player: no catch-up needed, it starts at the spawn
            // formula and is driven by local inputs. Mark so we don't
            // revisit it every frame.
            commands.entity(entity).insert(CatchUpRequested);
            continue;
        }
        if tick <= start.0 {
            // Remote player whose physics hasn't started yet: both
            // sides will begin simulating from an identical empty
            // state at `start.0`, so catch-up is unnecessary.
            debug!(
                ?entity,
                ?player_id,
                ?tick,
                ?start,
                "skipping catch-up for on-time remote player"
            );
            commands.entity(entity).insert(CatchUpRequested);
            continue;
        }
        debug!(
            ?entity,
            ?player_id,
            ?tick,
            ?start,
            "inserting PendingCatchUp on late-join remote player"
        );
        commands
            .entity(entity)
            .insert((PendingCatchUp, CatchUpRequested));
    }
}

#[derive(Component)]
struct CatchUpRequested;

/// Insert `CatchUpReady` as soon as the remote input buffer for a
/// `PendingCatchUp` entity has received any real remote inputs.
///
/// Each input rebroadcast batch carries roughly `HISTORY_DEPTH` ticks of
/// history, so as soon as `last_remote_tick` becomes `Some(_)` the client
/// has a buffered window of real inputs covering the server's most recent
/// ticks. That is enough for the post-catch-up rollback to replay from
/// the snapshot tick `S` forward using buffered inputs.
fn mark_catch_up_ready(
    mut commands: Commands,
    client: Option<Single<(), (With<Client>, With<IsSynced<InputTimeline>>)>>,
    pending: Query<
        (Entity, &LeafwingBuffer<PlayerActions>),
        (With<PendingCatchUp>, Without<CatchUpReady>),
    >,
) {
    if client.is_none() {
        return;
    }
    for (entity, buffer) in pending.iter() {
        let Some(last_remote_tick) = buffer.last_remote_tick else {
            continue;
        };
        debug!(?entity, ?last_remote_tick, "marking CatchUpReady");
        commands.entity(entity).insert(CatchUpReady);
    }
}

fn activate_physics_at_tick(
    client: Option<Single<&LocalId, (With<Client>, With<IsSynced<InputTimeline>>)>>,
    timeline: Res<LocalTimeline>,
    mut commands: Commands,
    pending: Query<
        (Entity, &PlayerId, &PhysicsStartTick, Option<&Position>),
        Without<PhysicsActivated>,
    >,
) {
    let Some(client) = client else {
        return;
    };
    let local_id = client.into_inner();
    let tick = timeline.tick();
    for (entity, player_id, start, existing_position) in pending.iter() {
        if tick < start.0 {
            continue;
        }
        let is_local = local_id.0 == player_id.0;
        let late_join = tick > start.0;
        // For remote late-join entities we cannot activate physics until the
        // catch-up snapshot has landed (Position is present as a confirmed
        // value). Adding physics earlier would run the simulation from the
        // wrong starting state until the rollback fires, causing spurious
        // ball/wall interactions.
        if late_join && !is_local && existing_position.is_none() {
            continue;
        }
        info!(
            "Client: activating physics for player {:?} at tick {:?} (scheduled {:?}, late_join={})",
            player_id.0, tick, start.0, late_join
        );
        let mut entity_mut = commands.entity(entity);
        entity_mut.insert((
            PhysicsBundle::player(),
            ColorComponent(color_from_id(player_id.0)),
            Name::from("Player"),
            // `skip_despawn: false` so that rollback doesn't insert
            // `DisableRollback` on the freshly-activated player during the
            // `enable_rollback_after` window: that would hide the player
            // from the client's checksum while the server (which has no
            // rollback) keeps hashing it, producing ~20 ticks of
            // guaranteed mismatches after every activation.
            DeterministicPredicted {
                skip_despawn: false,
                ..default()
            },
            PhysicsActivated,
        ));
        // For on-time activation, set Position from the spawn formula.
        // For remote late-join, Position has already arrived via the
        // catch-up snapshot and was written to PredictionHistory as
        // confirmed state by `add_confirmed_write`; state rollback will
        // snap Position back to that confirmed value at tick S and
        // replay forward using the buffered remote inputs.
        if !late_join || (is_local && existing_position.is_none()) {
            let y = (player_id.0.to_bits() as f32 * 50.0) % 500.0 - 250.0;
            entity_mut.insert(Position::from(Vec2::new(-50.0, y)));
        }
        if is_local {
            entity_mut.insert(InputMap::new([
                (PlayerActions::Up, KeyCode::KeyW),
                (PlayerActions::Down, KeyCode::KeyS),
                (PlayerActions::Left, KeyCode::KeyA),
                (PlayerActions::Right, KeyCode::KeyD),
            ]));
        }
        // For remote late-join we requested a catch-up snapshot and it
        // has just landed (Position is present). Schedule a one-shot
        // rollback to the server tick at which the snapshot was
        // produced. With `rollback_policy.state = Disabled` the
        // prediction system would otherwise never re-run from that
        // confirmed state, and the client would diverge forever.
        if late_join && !is_local && existing_position.is_some() {
            commands.queue(move |world: &mut World| {
                lightyear_deterministic_replication::prelude::request_forced_rollback_from_confirm_history(
                    world, entity,
                );
            });
        }
    }
}

