//! The client plugin.
use crate::shared::*;
use bevy::prelude::*;
use core::net::Ipv4Addr;
use core::net::{IpAddr, SocketAddr};
use lightyear::netcode::Key;
use lightyear::prelude::client::*;
use lightyear::prelude::*;

pub struct ExampleClientPlugin;

const CLIENT_ADDR: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 4000);

impl Plugin for ExampleClientPlugin {
    fn build(&self, app: &mut App) {
        // add our client-specific logic. Here we will just connect to the server
        app.add_systems(Startup, startup);
    }
}

/// Spawn a client that connects to the server
fn startup(mut commands: Commands) -> Result {
    commands.spawn(Camera2d);
    let auth = Authentication::Manual {
        server_addr: SERVER_ADDR,
        client_id: 0,
        private_key: Key::default(),
        protocol_id: 0,
    };
    let client = commands
        .spawn((
            Client::default(),
            LocalAddr(CLIENT_ADDR),
            PeerAddr(SERVER_ADDR),
            Link::new(None),
            ReplicationReceiver::default(),
            NetcodeClient::new(auth, NetcodeConfig::default())?,
            UdpIo::default(),
        ))
        .id();
    commands.trigger(Connect { entity: client });
    Ok(())
}
