//! The server side of the example.
//! It is possible (and recommended) to run the server in headless mode (without any rendering plugins).
//!
//! The server will:
//! - spawn a new player entity for each client that connects
//! - read inputs from the clients and move the player entities accordingly
//!
//! Lightyear will handle the replication of entities automatically if you add a `Replicate` component to them.
use bevy::prelude::*;
use bevy::utils::Duration;
use bevy::utils::HashMap;

use lightyear::prelude::server::*;
use lightyear::prelude::*;

use crate::protocol::*;
use crate::shared;
use crate::shared::shared_movement_behaviour;

pub struct ExampleServerPlugin;

impl Plugin for ExampleServerPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(Lobbies::default());
        app.add_systems(
            Startup,
            // start the dedicated server immediately (but not host servers)
            start_dedicated_server.run_if(SharedConfig::is_mode_separate),
        );
        app.add_systems(
            FixedUpdate,
            game::movement.run_if(in_state(NetworkingState::Started)),
        );
        app.add_systems(
            Update,
            game::handle_disconnections.run_if(in_state(NetworkingState::Started)),
        );
        app.add_systems(
            Update,
            (
                // in HostServer mode, we will spawn a player when a client connects
                game::handle_connections
            )
                .run_if(SharedConfig::is_host_server_condition),
        );
        app.add_systems(
            Update,
            // the lobby systems are only called on the dedicated server
            (
                lobby::handle_lobby_join,
                lobby::handle_lobby_exit,
                lobby::handle_start_game,
            )
                .run_if(SharedConfig::is_mode_separate),
        );
    }
}

#[derive(Resource)]
pub(crate) struct Global {
    pub client_id_to_entity_id: HashMap<ClientId, Entity>,
}

/// System to start the dedicated server at Startup
fn start_dedicated_server(mut commands: Commands) {
    commands.replicate_resource::<Lobbies, Channel1>(NetworkTarget::All);
    commands.start_server();
}

/// Spawn an entity for a given client
fn spawn_player_entity(
    commands: &mut Commands,
    client_id: ClientId,
    dedicated_server: bool,
) -> Entity {
    let replicate = Replicate {
        prediction_target: NetworkTarget::Single(client_id),
        interpolation_target: NetworkTarget::AllExceptSingle(client_id),
        controlled_by: NetworkTarget::Single(client_id),
        visibility: if dedicated_server {
            VisibilityMode::InterestManagement
        } else {
            VisibilityMode::All
        },
        ..default()
    };
    let entity = commands.spawn((PlayerBundle::new(client_id, Vec2::ZERO), replicate));
    info!("Create entity {:?} for client {:?}", entity.id(), client_id);
    entity.id()
}

mod game {
    use super::*;

    /// When a player connects, create a new player entity.
    /// This is only for the HostServer mode (for the dedicated server mode, the clients are already connected to the server
    /// to join the lobby list)
    pub(crate) fn handle_connections(
        mut connections: EventReader<ConnectEvent>,
        server: ResMut<ConnectionManager>,
        mut commands: Commands,
    ) {
        for connection in connections.read() {
            spawn_player_entity(&mut commands, connection.client_id, false);
        }
    }

    /// Delete the player's entity when the client disconnects
    pub(crate) fn handle_disconnections(
        mut disconnections: EventReader<DisconnectEvent>,
        server: ResMut<ConnectionManager>,
        mut lobbies: Option<ResMut<Lobbies>>,
    ) {
        for disconnection in disconnections.read() {
            // NOTE: games hosted by players will disappear from the lobby list since the host
            //  is not connected anymore
            if let Some(lobbies) = lobbies.as_mut() {
                lobbies.remove_client(disconnection.client_id);
            }
        }
    }

    /// Read client inputs and move players
    pub(crate) fn movement(
        mut position_query: Query<(&Replicate, &mut PlayerPosition)>,
        mut input_reader: EventReader<InputEvent<Inputs>>,
        tick_manager: Res<TickManager>,
    ) {
        for input in input_reader.read() {
            let client_id = input.context();
            if let Some(input) = input.input() {
                trace!(
                    "Receiving input: {:?} from client: {:?} on tick: {:?}",
                    input,
                    client_id,
                    tick_manager.tick()
                );
                // NOTE: you can define a mapping from client_id to entity_id to avoid iterating through all
                //  entities here
                for (replicate, position) in position_query.iter_mut() {
                    if replicate.controlled_by.targets(client_id) {
                        shared_movement_behaviour(position, input);
                    }
                }
            }
        }
    }
}

mod lobby {
    use lightyear::server::connection::ConnectionManager;
    use lightyear::server::visibility::room::RoomManager;

    use super::*;

    /// A client has joined a lobby:
    /// - update the `Lobbies` resource
    /// - add the Client to the room corresponding to the lobby
    pub(super) fn handle_lobby_join(
        mut events: EventReader<MessageEvent<JoinLobby>>,
        mut lobbies: ResMut<Lobbies>,
        mut room_manager: ResMut<RoomManager>,
        mut commands: Commands,
    ) {
        for lobby_join in events.read() {
            let client_id = *lobby_join.context();
            let lobby_id = lobby_join.message().lobby_id;
            info!("Client {client_id:?} joined lobby {lobby_id:?}");
            let lobby = lobbies.lobbies.get_mut(lobby_id).unwrap();
            lobby.players.push(client_id);
            room_manager.add_client(client_id, RoomId(lobby_id as u64));
            if lobby.in_game {
                // if the game has already started, we need to spawn the player entity
                let entity = spawn_player_entity(&mut commands, client_id, true);
                room_manager.add_entity(entity, RoomId(lobby_id as u64));
            }
        }
        // always make sure that there is an empty lobby for players to join
        if !lobbies.has_empty_lobby() {
            lobbies.lobbies.push(Lobby::default());
        }
    }

    /// A client has exited a lobby:
    /// - update the `Lobbies` resource
    /// - remove the Client from the room corresponding to the lobby
    pub(super) fn handle_lobby_exit(
        mut events: EventReader<MessageEvent<ExitLobby>>,
        mut lobbies: ResMut<Lobbies>,
        mut room_manager: ResMut<RoomManager>,
    ) {
        for lobby_join in events.read() {
            let client_id = lobby_join.context();
            let lobby_id = lobby_join.message().lobby_id;
            room_manager.remove_client(*client_id, RoomId(lobby_id as u64));
            lobbies.remove_client(*client_id);
        }
    }

    /// The game starts; if the host of the game is the dedicated server, we will spawn a cube
    /// for each player in the lobby
    pub(super) fn handle_start_game(
        mut connection_manager: ResMut<ConnectionManager>,
        mut events: EventReader<MessageEvent<StartGame>>,
        mut lobbies: ResMut<Lobbies>,
        mut room_manager: ResMut<RoomManager>,
        mut commands: Commands,
    ) {
        for event in events.read() {
            let client_id = event.context();
            let lobby_id = event.message().lobby_id;
            let host = event.message().host;
            let lobby = lobbies.lobbies.get_mut(lobby_id).unwrap();

            if !lobby.in_game {
                lobby.in_game = true;
                if let Some(host) = host {
                    lobby.host = Some(host);
                }
            }

            let room_id = RoomId(lobby_id as u64);
            // the client was not part of the lobby, they are joining in the middle of the game
            if !lobby.players.contains(client_id) {
                lobby.players.push(*client_id);
                if host.is_none() {
                    let entity = spawn_player_entity(&mut commands, *client_id, true);
                    room_manager.add_entity(entity, room_id);
                    room_manager.add_client(*client_id, room_id);
                }
                // send the StartGame message to the client who is trying to join the game
                let _ = connection_manager.send_message::<Channel1, _>(
                    *client_id,
                    &StartGame {
                        lobby_id,
                        host: lobby.host,
                    },
                );
            } else {
                if host.is_none() {
                    // one of the players asked for the game to start
                    for player in &lobby.players {
                        error!("Spawning player {player:?} entity for game");
                        let entity = spawn_player_entity(&mut commands, *player, true);
                        room_manager.add_entity(entity, room_id);
                    }
                }
                // redirect the StartGame message to all other clients in the lobby
                let _ = connection_manager.send_message_to_target::<Channel1, _>(
                    &StartGame {
                        lobby_id,
                        host: lobby.host,
                    },
                    NetworkTarget::Only(lobby.players.clone()),
                );
            }
        }
    }
}
