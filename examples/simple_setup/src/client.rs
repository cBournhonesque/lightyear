//! The client plugin.
use crate::shared::*;
use bevy::prelude::*;
use core::net::Ipv4Addr;
use core::net::{IpAddr, SocketAddr};
use lightyear::prelude::client::*;
use lightyear::prelude::*;

pub struct ExampleClientPlugin;

const CLIENT_ADDR: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 4000);

impl Plugin for ExampleClientPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, startup);
    }
}

fn startup(mut commands: Commands) {
    // spawn a client entity that will connect to the server
    let mut client = commands.spawn((
        // mark the entity as a 'Client' role
        Client,
        // you need to specify the local and remote addresses for the link
        LocalAddr(CLIENT_ADDR),
        PeerAddr(SERVER_ADDR),
        Link::default(),
        // allows this link to receive replication messages from the server
        ReplicationReceiver,
        // Without an explicit backend, the default debug connection uses transport-derived IDs.
        // Use NetcodeClient when authenticated identity is required.
        // the transport used to send and receive bytes over the network
        UdpIo::default(),
    ));
    client.trigger(Connect::from);
}
