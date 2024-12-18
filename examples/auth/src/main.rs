//! This example showcases how to send a ConnectToken securely from a game server to the client.
//!
//! Lightyear requires the client to have a ConnectToken to connect to the server. Normally the client
//! would get it from a backend server (for example via a HTTPS connection to a webserver).
//! If you don't have a separated backend server, you can use the game server to generate the ConnectToken.
//!
//! Run with
//! - `cargo run -- server`
//! - `cargo run -- client -c 1`
#![allow(unused_imports)]
#![allow(unused_variables)]
#![allow(dead_code)]

use crate::client::ExampleClientPlugin;
use crate::server::ExampleServerPlugin;
use crate::shared::SharedPlugin;
use bevy::prelude::*;
use lightyear_examples_common::app::{Apps, Cli};
use lightyear_examples_common::settings::{read_settings, Settings};
use serde::{Deserialize, Serialize};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};

mod client;
mod protocol;
mod server;
mod shared;

fn main() {
    let cli = Cli::default();
    let settings_str = include_str!("../assets/settings.ron");
    let settings = read_settings::<MySettings>(settings_str);
    // create user plugins
    let client_plugin = ExampleClientPlugin {
        auth_backend_address: SocketAddr::V4(SocketAddrV4::new(
            Ipv4Addr::UNSPECIFIED,
            settings.netcode_auth_port,
        )),
    };
    let server_plugin = ExampleServerPlugin {
        protocol_id: settings.common.shared.protocol_id,
        private_key: settings.common.shared.private_key,
        game_server_addr: SocketAddr::V4(SocketAddrV4::new(
            settings.common.client.server_addr,
            settings.common.client.server_port,
        )),
        auth_backend_addr: SocketAddr::V4(SocketAddrV4::new(
            Ipv4Addr::UNSPECIFIED,
            settings.netcode_auth_port,
        )),
    };
    // build the bevy app (this adds common plugin such as the DefaultPlugins)
    let mut apps = Apps::new(settings.common, cli, env!("CARGO_PKG_NAME").to_string());
    // add the lightyear [`ClientPlugins`] and [`ServerPlugins`]
    apps.add_lightyear_plugins()
        // add user plugins
        .add_user_plugins(client_plugin, server_plugin, SharedPlugin);
    // run the app
    apps.run();
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct MySettings {
    pub(crate) common: Settings,

    /// The server will listen on this port for incoming tcp authentication requests
    /// and respond with a [`ConnectToken`](lightyear::prelude::ConnectToken)
    pub(crate) netcode_auth_port: u16,
}
