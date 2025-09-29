use bevy::prelude::*;
use lightyear::prelude::{input::native::ActionState, *};

use crate::protocol::*;

#[derive(Clone)]
pub struct SharedPlugin;

impl Plugin for SharedPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(ProtocolPlugin);
        app.add_systems(Update, spawn_player);
    }
}

// Generate pseudo-random color from id
pub(crate) fn color_from_id(client_id: PeerId) -> Color {
    let h = (((client_id.to_bits().wrapping_mul(90)) % 360) as f32) / 360.0;
    let s = 1.0;
    let l = 0.5;
    Color::hsl(h, s, l)
}

// This system defines how we update the player's positions when we receive an input
pub(crate) fn shared_movement_behaviour(mut position: Mut<PlayerPosition>, input: &Inputs) {
    const MOVE_SPEED: f32 = 10.0;
    if let Inputs::Direction(direction) = input {
        if direction.up {
            position.y += MOVE_SPEED;
        }
        if direction.down {
            position.y -= MOVE_SPEED;
        }
        if direction.left {
            position.x -= MOVE_SPEED;
        }
        if direction.right {
            position.x += MOVE_SPEED;
        }
    }
}


/// Spawn a client-owned player entity when the space command is pressed
fn spawn_player(
    mut commands: Commands,
    is_server: Query<Has<Server>>,
    clients: Query<(Entity, &ActionState<Inputs>, &LocalId, &RemoteId)>,
) {
    let is_server = is_server.single().is_ok();
    clients.iter().for_each(|(client_entity,input, local_id, remote_id)| {
        // TODO: switch to leafwing or BEI for just_pressed
        if input.0 == Inputs::Spawn {
            let client_id = if is_server {
                remote_id.0
            } else {
                local_id.0
            };
            info!(
                "Spawning client-owned player entity for client: {}",
                client_id
            );

            let mut entity = commands.spawn((
                Name::from("Player"),
                PlayerId(client_id),
                PlayerPosition(Vec2::ZERO),
                PlayerColor(color_from_id(client_id)),
                // This will let the server match the replicated entity
                PreSpawned::default(),
            ));
            if is_server {
                #[cfg(feature = "server")]
                entity.insert((
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

        }
    });
}
