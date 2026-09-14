use crate::shared::*;
use bevy::prelude::*;
use lightyear::prelude::client::*;
use lightyear::prelude::server::*;
use lightyear::prelude::*;

pub struct ExampleServerPlugin;

impl Plugin for ExampleServerPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, startup);
        app.add_observer(handle_new_client);
    }
}

/// Whenever a new client connects to the server, a new entity will get spawned in the server's world
/// with a [`Link`] component to exchange bytes with the client.
///
/// It will also have some other components:
/// - [`Connected`] (or [`Disconnected`]) to track the connection state of the link
/// - [`LinkOf`] (relationship that tracks which server the link is associated with)
///
/// You can add more components to customize how this connection, for example by adding a
/// [`ReplicationSender`] (which means that the server will replicate the state of the world to this client)
fn handle_new_client(trigger: On<Add, Connected>, mut commands: Commands) {
    commands.entity(trigger.entity).insert(ReplicationSender);

    // spawn an entity for this client, that we will replicate to all clients
    commands.spawn((
        PlayerPosition::default(),
        // this entity has a link to the client
        Replicate::to_clients(NetworkTarget::All),
    ));
}

fn startup(mut commands: Commands) -> Result {
    // start a server entity
    let server = commands
        .spawn((
            // Links need a 'connection' component that provides them with a durable entity beyond a simple IP address.
            // Usually you would use something like NetcodeServer or SteamServer, but for this example
            // we will use [`RawServer`], which uses the IP as the durable network identifier for the entity
            RawServer,
            // you need to specify the address to bind the server to
            LocalAddr(SERVER_ADDR),
            // the transport that we will use is Udp
            ServerUdpIo,
        ))
        .id();
    // you can use triggers to start/stop the server
    commands.trigger(Start { entity: server });
    Ok(())
}
