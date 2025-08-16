//! The server side of the example.
//! It is possible (and recommended) to run the server in headless mode (without any rendering plugins).
//!
//! The server will:
//! - spawn a new player entity for each client that connects
//! - read inputs from the clients and move the player entities accordingly
//!
//! Lightyear will handle the replication of entities automatically if you add a `Replicate` component to them.
use crate::shared::*;
use bevy::prelude::*;
use lightyear::prelude::server::*;
use lightyear::prelude::*;

pub struct ExampleServerPlugin;

impl Plugin for ExampleServerPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, startup);
        app.add_observer(handle_new_client);
    }
}

/// Whenever a new client connects to the server, a new entity will get spawned with
/// the `Connected` component, which represents the connection between the server and that specific client.
///
/// You can add more components to customize how this connection, for example by adding a
/// `ReplicationSender` (so that the server can send replication updates to that client)
/// or a `MessageSender`.
fn handle_new_client(trigger: Trigger<OnAdd, Connected>, mut commands: Commands) {
    commands
        .entity(trigger.target())
        .insert(ReplicationSender::new(
            SERVER_REPLICATION_INTERVAL,
            SendUpdatesMode::SinceLastAck,
            false,
        ));
    let mut found = vec![];
    for _ in 0..10 {
        found.push(
            commands
                .spawn((
                    Replicate::to_clients(NetworkTarget::All),
                    StressComponent {
                        entities: found.clone(),
                    },
                ))
                .id(),
        );
    }
}

/// Start the server
fn startup(mut commands: Commands) -> Result {
    let server = commands
        .spawn((
            NetcodeServer::new(NetcodeConfig::default()),
            LocalAddr(SERVER_ADDR),
            ServerUdpIo::default(),
        ))
        .id();
    commands.trigger_targets(Start, server);
    Ok(())
}

