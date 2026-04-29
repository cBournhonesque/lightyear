use avian2d::prelude::*;
use bevy::prelude::*;
use leafwing_input_manager::prelude::*;
use lightyear::prediction::rollback::DeterministicPredicted;
use lightyear::prelude::client::*;
use lightyear::prelude::*;

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
            add_input_map_after_sync.after(ReplicationSystems::Receive),
        );
        app.add_systems(FixedPreUpdate, activate_physics_when_bundle_lands);
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

/// Activate physics on every `CatchUpGated` entity once the bundled catch-up
/// snapshot has landed. `add_confirmed_write` has already written the
/// catch-up components (Position / Rotation / LinearVelocity /
/// AngularVelocity) into `PredictionHistory` at server tick `S`. We add the
/// `PhysicsBundle` (colliders etc.) + `DeterministicPredicted` so Avian will
/// simulate this entity locally. Once *every* gated entity we know about
/// has physics activated, fire a single forced rollback that reconciles
/// everything from `S` forward.
fn activate_physics_when_bundle_lands(
    mut commands: Commands,
    // Players whose catch-up snapshot has just landed (they now have
    // `Position`) but we haven't yet added local physics components.
    pending: Query<(Entity, &PlayerId, &Position), Without<PhysicsActivated>>,
    // Known remote players that are still waiting for the bundled snapshot
    // (they have `PlayerId` from structural replication but no `Position`
    // yet). The `still_pending` guard ensures the forced rollback fires
    // only when the *entire* bundle has arrived.
    still_pending: Query<Entity, (With<PlayerId>, Without<PhysicsActivated>, Without<Position>)>,
) {
    let mut newly_activated: Vec<Entity> = Vec::new();
    for (entity, player_id, _position) in pending.iter() {
        info!(
            "Client: activating physics for player {:?} (catch-up bundle snapshot landed)",
            player_id.0
        );
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
        newly_activated.push(entity);
    }
    // Only fire the single forced rollback once ALL gated players we know
    // about have their catch-up components. Replicon emits the bundle in
    // one update at one tick `S`, so in practice every player gets
    // `Position` on the same frame and the rollback fires once.
    if !newly_activated.is_empty() && still_pending.is_empty() {
        // Pick any activated entity as the reference for the catch-up
        // tick lookup — they all share the same server tick in the
        // bundled snapshot.
        let reference = newly_activated[0];
        commands.queue(move |world: &mut World| {
            lightyear_deterministic_replication::prelude::request_forced_rollback_to_catch_up_tick(
                world, reference,
            );
        });
    }
}
