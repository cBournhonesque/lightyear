//! The server side of the example.
//! It is possible (and recommended) to run the server in headless mode (without any rendering plugins).
//!
//! The server will:
//! - spawn a new player entity for each client that connects
//! - read inputs from the clients and move the player entities accordingly
//!
//! Lightyear will handle the replication of entities automatically if you add a `Replicate` component to them.
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use core::time::Duration;

use crate::protocol::*;
use crate::shared;
use crate::shared::shared_movement_behaviour;
use lightyear::prelude::server::*;
use lightyear::prelude::*;
use lightyear_examples_common::shared::SEND_INTERVAL;

pub struct ExampleServerPlugin {
    pub(crate) is_dedicated_server: bool,
}

impl Plugin for ExampleServerPlugin {
    fn build(&self, app: &mut App) {
        app.world_mut().spawn(Lobbies::default());

        // start the dedicated server immediately (but not host servers)
        if self.is_dedicated_server {
            app.add_systems(Startup, start_dedicated_server);
        }
        app.add_observer(handle_new_client);
        app.add_systems(FixedUpdate, game::movement);
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
                .run_if(is_host_server),
        );
        if self.is_dedicated_server {
            app.add_systems(
                Update,
                // the lobby systems are only called on the dedicated server
                (
                    lobby::handle_lobby_join,
                    lobby::handle_lobby_exit,
                    lobby::handle_start_game,
                ),
            );
        }
    }
}

/// System to start the dedicated server at Startup
fn start_dedicated_server(mut commands: Commands) {
    commands.spawn((
        Lobbies::default(),
        Replicate::to_clients(NetworkTarget::All),
    ));
}

pub(crate) fn handle_new_client(trigger: Trigger<OnAdd, LinkOf>, mut commands: Commands) {
    commands.entity(trigger.target()).insert((
        ReplicationSender::new(SEND_INTERVAL, SendUpdatesMode::SinceLastAck, false),
        Name::from("Client"),
    ));
}

/// Spawn an entity for a given client
fn spawn_player_entity(
    commands: &mut Commands,
    client_entity: Entity,
    client_id: PeerId,
    dedicated_server: bool,
) -> Entity {
    let h = (((client_id.to_bits().wrapping_mul(30)) % 360) as f32) / 360.0;
    let s = 0.8;
    let l = 0.5;
    let color = Color::hsl(h, s, l);
    let entity = commands
        .spawn((
            PlayerId(client_id),
            PlayerPosition(Vec2::ZERO),
            PlayerColor(color),
            Replicate::to_clients(NetworkTarget::All),
            PredictionTarget::to_clients(NetworkTarget::Single(client_id)),
            InterpolationTarget::to_clients(NetworkTarget::AllExceptSingle(client_id)),
            ControlledBy {
                owner: client_entity,
                lifetime: Default::default(),
            },
        ))
        .id();
    if dedicated_server {
        commands.entity(entity).insert(NetworkVisibility::default());
    }
    info!("Create entity {:?} for client {:?}", entity, client_id);
    entity
}

mod game {
    use super::*;
    use lightyear::input::native::prelude::ActionState;

    /// When a player connects, create a new player entity.
    /// This is only for the HostServer mode (for the dedicated server mode, the clients are already connected to the server
    /// to join the lobby list)
    pub(crate) fn handle_connections(
        trigger: Trigger<OnAdd, Connected>,
        query: Query<&Connected, With<ClientOf>>,
        mut commands: Commands,
    ) {
        let Ok(connected) = query.get(trigger.target()) else {
            return;
        };
        let client_id = connected.remote_peer_id;
        info!("HostServer spawn player for client {client_id:?}");
        spawn_player_entity(&mut commands, trigger.target(), client_id, false);
    }

    /// Delete the player's entity when the client disconnects
    pub(crate) fn handle_disconnections(
        // TODO: need a way to get the peer_id that disconnected
        mut disconnections: Trigger<OnAdd, Disconnected>,
        mut lobbies: Single<&mut Lobbies>,
    ) {
        // NOTE: games hosted by players will disappear from the lobby list since the host
        //  is not connected anymore
        lobbies.remove_client(disconnection.client_id);
    }

    /// Read client inputs and move players
    pub(crate) fn movement(
        server_started: Single<(), (With<Server>, With<Started>)>,
        mut position_query: Query<(&mut PlayerPosition, &ActionState<Inputs>)>,
    ) {
        for (position, inputs) in position_query.iter_mut() {
            if let Some(inputs) = &inputs.value {
                shared_movement_behaviour(position, inputs);
            }
        }
    }
}

mod lobby {
    use super::*;

    /// A client has joined a lobby:
    /// - update the `Lobbies` resource
    /// - add the Client to the room corresponding to the lobby
    pub(super) fn handle_lobby_join(
        mut events: EventReader<ServerReceiveMessage<JoinLobby>>,
        mut lobbies: ResMut<Lobbies>,
        mut room_manager: ResMut<RoomManager>,
        mut commands: Commands,
    ) {
        for lobby_join in events.read() {
            let client_id = lobby_join.from();
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
        mut events: Query<(Entity, &Connected, &mut MessageReceiver<StartGame>)>,
        mut lobbies: Single<&mut Lobbies>,
        mut commands: Commands,
    ) {
        for (sender, connected, mut receiver) in events.iter_mut() {
            let client_id = connected.remote_peer_id;
            for event in receiver.receive() {
                let lobby_id = lobby_join.lobby_id;
                commands.trigger_targets(RoomEvent::AddSender(sender), room_id);
                room_manager.remove_client(client_id, RoomId(lobby_id as u64));
                lobbies.remove_client(client_id);
            }
        }
    }

    /// The game starts; if the host of the game is the dedicated server, we will spawn a cube
    /// for each player in the lobby
    pub(super) fn handle_start_game(
        server: Single<&Server>,
        mut events: Query<(Entity, &Connected, &mut MessageReceiver<StartGame>)>,
        mut multi_sender: ServerMultiMessageSender,
        mut lobbies: Single<&mut Lobbies>,
        mut commands: Commands,
    ) -> Result {
        let server = server.into_inner();
        for (sender, connected, mut receiver) in events.iter_mut() {
            let client_id = connected.remote_peer_id;
            for event in receiver.receive() {
                let lobby_id = event.lobby_id;
                let host = event.host;
                let lobby = lobbies.lobbies.get_mut(lobby_id).unwrap();

                // Setting lobby ingame
                if !lobby.in_game {
                    lobby.in_game = true;
                    if let Some(host) = host {
                        lobby.host = Some(host);
                    }
                }

                let room_id = commands.spawn(Room::default()).id();
                // the client was not part of the lobby, they are joining in the middle of the game
                if !lobby.players.contains(&client_id) {
                    lobby.players.push(client_id);
                    if host.is_none() {
                        let entity = spawn_player_entity(&mut commands, sender, client_id, true);
                        commands.trigger_targets(RoomEvent::AddEntity(entity), room_id);
                        commands.trigger_targets(RoomEvent::AddSender(sender), room_id);
                    }
                    multi_sender.send::<_, Channel1>(
                        &StartGame {
                            lobby_id,
                            host: lobby.host,
                        },
                        server,
                        &NetworkTarget::Single(client_id),
                    )?;
                } else {
                    if host.is_none() {
                        // one of the players asked for the game to start
                        for player in &lobby.players {
                            info!("Spawning player  {player:?} in server hosted  game");
                            let entity = spawn_player_entity(&mut commands, sender, *player, true);
                            commands.trigger_targets(RoomEvent::AddEntity(entity), room_id);
                        }
                    }
                    // redirect the StartGame message to all other clients in the lobby
                    multi_sender.send::<_, Channel1>(
                        &StartGame {
                            lobby_id,
                            host: lobby.host,
                        },
                        server,
                        &NetworkTarget::Only(lobby.players.clone()),
                    )?;
                }
            }
        }
        Ok(())
    }
}
