use avian2d::prelude::*;
use bevy::prelude::*;
use leafwing_input_manager::prelude::*;
use lightyear::input::leafwing::prelude::LeafwingBuffer;
use lightyear::prediction::rollback::DeterministicPredicted;
use lightyear::prelude::client::*;
use lightyear::prelude::*;
use lightyear_deterministic_replication::prelude::{
    CatchUpReady, LateJoinCatchUpPlugin, PendingCatchUp,
};

use crate::automation::AutomationClientPlugin;
use crate::protocol::*;
use crate::shared::{self, GameStartMode, color_from_id, player_bundle};

/// How close the remote input buffer must be to the current `RemoteTimeline`
/// tick before we request a catch-up snapshot. Ticks inside this margin are
/// still considered "covered" because rebroadcast inputs arrive slightly
/// behind the remote's current tick.
const CATCH_UP_READINESS_MARGIN: i32 = 4;

pub struct ExampleClientPlugin;

impl Plugin for ExampleClientPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(AutomationClientPlugin);
        if !app
            .is_plugin_added::<lightyear_deterministic_replication::prelude::ChecksumSendPlugin>()
        {
            app.add_plugins(lightyear_deterministic_replication::prelude::ChecksumSendPlugin);
        }
        app.add_plugins(LateJoinCatchUpPlugin::<Channel1>::default());
        app.add_systems(
            PreUpdate,
            (add_input_map_after_sync, request_catch_up_for_remote_players)
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

/// Mark every *remote* player entity (not our own) as waiting for a
/// late-join catch-up snapshot.
///
/// We only insert `PendingCatchUp` once per entity: a `CatchUpRequested`
/// marker is added at the same time so subsequent runs skip the entity.
fn request_catch_up_for_remote_players(
    client: Option<Single<&LocalId, (With<Client>, With<IsSynced<InputTimeline>>)>>,
    mut commands: Commands,
    players: Query<(Entity, &PlayerId), Without<CatchUpRequested>>,
) {
    let Some(client) = client else {
        return;
    };
    let local_id = client.into_inner();
    for (entity, player_id) in players.iter() {
        if local_id.0 == player_id.0 {
            // Local player: no catch-up needed, it starts at the spawn
            // formula and is driven by local inputs. Mark so we don't
            // revisit it every frame.
            commands.entity(entity).insert(CatchUpRequested);
            continue;
        }
        commands
            .entity(entity)
            .insert((PendingCatchUp, CatchUpRequested));
    }
}

#[derive(Component)]
struct CatchUpRequested;

/// When the remote input buffer for a `PendingCatchUp` entity has caught up
/// to roughly the current remote tick, insert `CatchUpReady` so the plugin
/// sends the catch-up message.
///
/// We intentionally use the strict "covers ~current remote tick" condition
/// so that by the time the server replies with the snapshot at tick S, the
/// client already has inputs for `[S, now]` in its rebroadcast buffer.
fn mark_catch_up_ready(
    mut commands: Commands,
    remote_timeline: Option<
        Single<&RemoteTimeline, (With<Client>, With<IsSynced<InputTimeline>>)>,
    >,
    pending: Query<
        (Entity, &LeafwingBuffer<PlayerActions>),
        (With<PendingCatchUp>, Without<CatchUpReady>),
    >,
) {
    let Some(remote_timeline) = remote_timeline else {
        return;
    };
    let remote_tick = remote_timeline.into_inner().tick();
    for (entity, buffer) in pending.iter() {
        let Some(end_tick) = buffer.end_tick() else {
            continue;
        };
        if end_tick + CATCH_UP_READINESS_MARGIN >= remote_tick {
            commands.entity(entity).insert(CatchUpReady);
        }
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
            DeterministicPredicted {
                skip_despawn: true,
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
    }
}
