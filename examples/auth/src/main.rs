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

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};

use lightyear_examples_common::{
    app::{Apps, Cli},
    settings::Settings,
};
use serde::{Deserialize, Serialize};

use crate::shared::SharedPlugin;

#[cfg(feature = "client")]
mod client;
mod protocol;

#[cfg(feature = "gui")]
mod renderer;
#[cfg(feature = "server")]
mod server;
mod settings;
mod shared;

fn main() {
    let cli = Cli::default();
    let settings = settings::get_settings();

    #[cfg(feature = "client")]
    let client_plugin = client::ExampleClientPlugin {
        auth_backend_address: SocketAddr::V4(SocketAddrV4::new(
            Ipv4Addr::UNSPECIFIED,
            settings.netcode_auth_port,
        )),
    };

    #[cfg(feature = "server")]
    let server_plugin = server::ExampleServerPlugin {
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

    apps.add_lightyear_plugins();
    apps.add_user_shared_plugin(SharedPlugin);
    #[cfg(feature = "client")]
    apps.add_user_client_plugin(client_plugin);
    #[cfg(feature = "server")]
    apps.add_user_server_plugin(server_plugin);
    #[cfg(feature = "gui")]
    apps.add_user_renderer_plugin(renderer::ExampleRendererPlugin);

    // run the app
    apps.run();
}
