//! This example showcases how to use Lightyear with Bevy, to easily get replication along with prediction/interpolation working.
//!
//! There is a lot of setup code, but it's mostly to have the examples work in all possible configurations of transport.
//! (all transports are supported, as well as running the example in listen-server or host-server mode)
//!
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
use common::app::Apps;
use common::settings::Settings;
use serde::{Deserialize, Serialize};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};

mod client;
mod protocol;
mod server;
mod shared;

fn main() {
    let cli = common::app::cli();
    let settings_str = include_str!("../assets/settings.ron");
    let settings = common::settings::settings::<MySettings>(settings_str);
    // build the bevy app (this adds common plugin such as the DefaultPlugins)
    // and returns the `ClientConfig` and `ServerConfig` so that we can modify them if needed
    let mut app = common::app::build_app(settings.common.clone(), cli);
    // add the lightyear [`ClientPlugins`] and [`ServerPlugins`]
    app.add_lightyear_plugin_groups();
    // add out plugins
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
    app.add_plugins(client_plugin, server_plugin, SharedPlugin);
    // run the app
    app.run();
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct MySettings {
    pub(crate) common: Settings,

    /// The server will listen on this port for incoming tcp authentication requests
    /// and respond with a [`ConnectToken`](lightyear::prelude::ConnectToken)
    pub(crate) netcode_auth_port: u16,
}
