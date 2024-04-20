//! The lobby server will listen for client connections and disconnections, and keep track of the players in the lobby.
use crate::protocol::*;
use bevy::prelude::*;
pub use lightyear::prelude::server::*;
use lightyear::prelude::*;

pub struct LobbyServerPlugin;

impl Plugin for LobbyServerPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(Lobby::default());
        app.add_systems(Startup, (init, start_server));
        app.add_systems(Update, (handle_connections, handle_disconnections));
    }
}

/// System to start the server at Startup
fn start_server(world: &mut World) {
    world
        .start_server::<MyProtocol>()
        .expect("Failed to start server");
}

/// System initializing the replication of the Lobby resource to all clients
fn init(mut commands: Commands) {
    commands.replicate_resource::<Lobby>(Replicate::default());
}

/// When a player gets connected, add them to the lobby
pub(crate) fn handle_connections(
    mut connections: EventReader<ConnectEvent>,
    mut lobby: ResMut<Lobby>,
) {
    for connection in connections.read() {
        let client_id = *connection.context();
        lobby.players.push(client_id);
    }
}

/// When a player gets disconnected, remove them from the lobby
pub(crate) fn handle_disconnections(
    mut disconnections: EventReader<DisconnectEvent>,
    mut lobby: ResMut<Lobby>,
) {
    for disconnection in disconnections.read() {
        let client_id = disconnection.context();
        lobby.players.retain(|x| x != client_id);
    }
}
