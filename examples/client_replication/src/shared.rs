use crate::protocol::*;
use bevy::prelude::*;
use bevy_enhanced_input::action::Action;
use bevy_enhanced_input::bindings;
use bevy_enhanced_input::prelude::{ActionOf, Bindings, Cardinal};
use lightyear::connection::client::PeerMetadata;
use lightyear::input::bei::prelude::{Complete, Fire};
use lightyear::prelude::*;

#[derive(Clone)]
pub struct SharedPlugin;

impl Plugin for SharedPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(ProtocolPlugin);
        app.add_observer(movement);
        app.add_observer(spawn_player);
        app.add_observer(despawn_player);
    }
}

// Generate pseudo-random color from id
pub(crate) fn color_from_id(client_id: PeerId) -> Color {
    let h = (((client_id.to_bits().wrapping_mul(90)) % 360) as f32) / 360.0;
    let s = 1.0;
    let l = 0.5;
    Color::hsl(h, s, l)
}

/// Read client inputs and move players
pub(crate) fn movement(
    trigger: On<Fire<Movement>>,
    mut position_query: Query<&mut PlayerPosition>,
) {
    if let Ok(mut position) = position_query.get_mut(trigger.context) {
        const MOVE_SPEED: f32 = 10.0;
        position.y += trigger.value.y * MOVE_SPEED;
        position.x += trigger.value.x * MOVE_SPEED;
    }
}

/// Spawn a client-owned player entity when the space command is pressed
fn spawn_player(
    trigger: On<Complete<SpawnPlayer>>,
    mut commands: Commands,
    is_server: Query<(), With<Server>>,
    clients: Query<&PlayerId>,
    peer_metadata: Option<Res<PeerMetadata>>,
) {
    let is_server = is_server.single().is_ok();
    if let Ok(player_id) = clients.get(trigger.context) {
        let client_id = player_id.0;
        info!(
            ?is_server,
            "Spawning client-owned player entity for client: {}", client_id
        );
        let mut entity_commands = commands.spawn((
            Name::from("Player"),
            Player,
            PlayerId(client_id),
            PlayerPosition(Vec2::ZERO),
            PlayerColor(color_from_id(client_id)),
            // This will let the server match the replicated entity
            PreSpawned::default(),
        ));

        if is_server {
            let client_entity = *peer_metadata.unwrap().mapping.get(&client_id).unwrap();
            #[cfg(feature = "server")]
            entity_commands.insert((
                // we want to replicate back to the original client, since they are using a pre-spawned entity
                Replicate::to_clients(NetworkTarget::All),
                // NOTE: even with a pre-spawned Predicted entity, we need to specify who will run prediction
                PredictionTarget::to_clients(NetworkTarget::Single(client_id)),
                InterpolationTarget::to_clients(NetworkTarget::AllExceptSingle(client_id)),
                ControlledBy {
                    owner: client_entity,
                    lifetime: Lifetime::SessionBased,
                },
            ));
        }

        let entity = entity_commands.id();
        let mut action = commands.spawn((
            ActionOf::<Player>::new(entity),
            Action::<Movement>::new(),
            Bindings::spawn(Cardinal::wasd_keys()),
            PreSpawned::default_with_salt(1),
        ));
        // For PreSpawned Contexts, the actions must be PreSpawned as well,
        // and replicated from server to client
        if is_server {
            #[cfg(feature = "server")]
            action.insert((
                Replicate::to_clients(NetworkTarget::Single(client_id)),
                // make sure that the context and action are replicated together
                PREDICTION_GROUP,
            ));
        }
        let mut action = commands.spawn((
            ActionOf::<Player>::new(entity),
            Action::<DespawnPlayer>::new(),
            bindings![KeyCode::KeyK,],
            PreSpawned::default_with_salt(2),
        ));
        if is_server {
            #[cfg(feature = "server")]
            action.insert((
                Replicate::to_clients(NetworkTarget::Single(client_id)),
                PREDICTION_GROUP,
            ));
        }
    }
}

/// Delete the predicted player when the space command is pressed
///
/// Make sure to use `prediction_despawn`: the entity will be temporarily be Disabled
/// until we receive a confirmation from the server that it should actually be despawned
fn despawn_player(trigger: On<Complete<DespawnPlayer>>, mut commands: Commands) {
    if let Ok(mut entity_mut) = commands.get_entity(trigger.context) {
        entity_mut.prediction_despawn();
        info!(
            "Despawning the player {:?} because we received player action!",
            trigger.context
        );
    }
}
