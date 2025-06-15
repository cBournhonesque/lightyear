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
        // the server is using Rooms
        app.add_plugins(RoomPlugin);
        
        app.add_observer(handle_new_client);
        app.add_systems(FixedUpdate, game::movement);
        app.add_observer(game::handle_disconnections);
        // app.add_systems(
        //     Update,
        //     (
        //         // in HostServer mode, we will spawn a player when a client connects
        //         game::handle_connections
        //     )
        //         .run_if(is_host_server),
        // );
        
        if self.is_dedicated_server {
            // start the dedicated server immediately (but not host servers)
            app.add_systems(Startup, start_dedicated_server);
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
    let mut lobbies = Lobbies::default();
    // add one empty lobby
    let room = commands.spawn((
        Room::default(),
        Name::from("Room"))
    ).id();
    lobbies.lobbies.push(Lobby::new(room));
    commands.spawn((
        Name::from("Lobbies"),
        lobbies,
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
            Name::from("Player"),
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
        query: Query<&RemoteId, With<ClientOf>>,
        mut commands: Commands,
    ) {
        let Ok(remote_id) = query.get(trigger.target()) else {
            return;
        };
        let client_id = remote_id.0;
        info!("HostServer spawn player for client {client_id:?}");
        spawn_player_entity(&mut commands, trigger.target(), client_id, false);
    }

    /// Delete the player's entity when the client disconnects
    pub(crate) fn handle_disconnections(
        trigger: Trigger<OnAdd, Disconnected>,
        query: Query<&RemoteId, With<ClientOf>>,
        mut lobbies: Single<&mut Lobbies>,
        mut commands: Commands,
    ) {
        if let Ok(remote_id) = query.get(trigger.target()) {
            info!("Client {remote_id:?} disconnected, removing from lobby");
            // NOTE: games hosted by players will disappear from the lobby list since the host
            //  is not connected anymore
            lobbies.remove_client(remote_id.0, &mut commands);
        }
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
        mut receiver: Query<(Entity, &RemoteId, &mut MessageReceiver<JoinLobby>)>,
        mut lobbies: Single<&mut Lobbies>,
        mut commands: Commands,
    ) {
        for (client_entity, remote_id, mut message_receiver) in receiver.iter_mut() {
            let client_id = remote_id.0;
            message_receiver.receive().for_each(|message| {
                let lobby_id = message.lobby_id;
                let lobby = lobbies.lobbies.get_mut(lobby_id).unwrap();
                let room = lobby.room;
                info!("Client {client_id:?} joined lobby {lobby_id:?}. Room: {room}");
                lobby.players.push(client_id);
                commands.trigger_targets(RoomEvent::AddSender(client_entity), room);
                if lobby.in_game {
                    // if the game has already started, we need to spawn the player entity
                    let entity = spawn_player_entity(&mut commands, client_entity, client_id, true);
                    commands.trigger_targets(RoomEvent::AddEntity(entity), room);
                }
                // always make sure that there is an empty lobby for players to join
                if !lobbies.has_empty_lobby() {
                    let room = commands.spawn(Room::default()).id();
                    lobbies.lobbies.push(Lobby::new(room));
                }
            })
        }
        
    }

    /// A client has exited a lobby:
    /// - update the `Lobbies` resource
    /// - remove the Client from the room corresponding to the lobby
    pub(super) fn handle_lobby_exit(
        mut events: Query<(Entity, &RemoteId, &mut MessageReceiver<ExitLobby>), With<Connected>>,
        mut lobbies: Single<&mut Lobbies>,
        mut commands: Commands,
    ) {
        for (sender, remote_id, mut receiver) in events.iter_mut() {
            let client_id = remote_id.0;
            for message in receiver.receive() {
                let lobby_id = message.lobby_id;
                info!("Client {client_id:?} exited lobby {lobby_id:?}");
                let room = lobbies.lobbies[lobby_id].room;
                commands.trigger_targets(RoomEvent::RemoveSender(sender), room);
                lobbies.remove_client(client_id, &mut commands);
            }
        }
    }

    /// The game starts; if the host of the game is the dedicated server, we will spawn a cube
    /// for each player in the lobby
    pub(super) fn handle_start_game(
        server: Single<&Server>,
        mut events: Query<(Entity, &RemoteId, &mut MessageReceiver<StartGame>), With<Connected>>,
        mut multi_sender: ServerMultiMessageSender,
        mut lobbies: Single<&mut Lobbies>,
        mut commands: Commands,
    ) -> Result {
        let server = server.into_inner();
        for (sender, remote_id, mut receiver) in events.iter_mut() {
            let client_id = remote_id.0;
            for event in receiver.receive() {
                info!("Received start game message! {event:?}");
                let lobby_id = event.lobby_id;
                let host = event.host;
                let lobby = lobbies.lobbies.get_mut(lobby_id).unwrap();

                // Setting lobby ingame
                if !lobby.in_game {
                    lobby.in_game = true;
                    lobby.host = host;
                }

                // the client was not part of the lobby, they are joining in the middle of the game
                if !lobby.players.contains(&client_id) {
                    info!("Receives start game for a player {client_id:?} who wasn't part of the lobby! They are joining in the middle of the game");
                    lobby.players.push(client_id);
                    if host.is_none() {
                        let entity = spawn_player_entity(&mut commands, sender, client_id, true);
                        commands.trigger_targets(RoomEvent::AddEntity(entity), lobby.room);
                        commands.trigger_targets(RoomEvent::AddSender(sender), lobby.room);
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
                        info!("Received start game for lobby {lobby_id:?}. Dedicated server is hosting.");
                        // one of the players asked for the game to start
                        for player in &lobby.players {
                            info!("Spawning player  {player:?} in server hosted  game");
                            let entity = spawn_player_entity(&mut commands, sender, *player, true);
                            commands.trigger_targets(RoomEvent::AddEntity(entity), lobby.room);
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
