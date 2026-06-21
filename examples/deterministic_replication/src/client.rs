use bevy::prelude::*;
use leafwing_input_manager::prelude::*;
use lightyear::prediction::rollback::{CatchUpGated, DeterministicPredicted};
use lightyear::prelude::client::*;
use lightyear::prelude::*;

use crate::automation::AutomationClientPlugin;
use crate::protocol::*;
use crate::shared::color_from_id;
use lightyear_deterministic_replication::prelude::CatchUpSnapshotReady;

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
        app.add_observer(activate_physics_when_bundle_lands);
    }
}

#[derive(Component)]
struct InputMapAdded;

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

/// Activate physics when replicated deterministic state lands.
///
/// During initial catch-up, the late-join plugin emits `CatchUpSnapshotReady`
/// after all gated state arrives and then automatically schedules the forced
/// rollback. For already-caught-up clients, the same event is emitted when a
/// new catch-up-gated entity's state becomes visible. The example only needs
/// to add local-only physics components in response.
fn activate_physics_when_bundle_lands(
    _trigger: On<CatchUpSnapshotReady>,
    mut commands: Commands,
    pending: Query<
        (Entity, &PlayerId),
        (
            With<CatchUpGated>,
            With<PlayerActivationTick>,
            Without<DeterministicPredicted>,
        ),
    >,
) {
    let mut ready = Vec::new();
    for (entity, player_id) in &pending {
        ready.push((entity, player_id.0));
    }

    // Avian's deterministic physics can depend on the order in which bodies
    // are inserted into its internal structures. Late-joining clients can
    // receive replicated players in a different order from the server, so
    // activate every ready player in a stable game-defined order.
    ready.sort_by_key(|(_, player_id)| player_id.to_bits());
    for (entity, player_id) in ready {
        info!(
            "Client: activating physics for player {:?} (deterministic snapshot landed)",
            player_id
        );
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
        ));
    }
}
