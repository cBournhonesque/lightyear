//! This example showcases how to use Lightyear with Bevy, to easily get replication along with prediction/interpolation working.
//!
//! There is a lot of setup code, but it's mostly to have the examples work in all possible configurations of transport.
//! (all transports are supported, as well as running the example in client-and-server or host-server mode)
//!
//!
//! Run with
//! - `cargo run -- server`
//! - `cargo run -- client -c 1`
#![allow(unused_imports)]
#![allow(unused_variables)]
#![allow(dead_code)]
use crate::settings::get_settings;
use crate::shared::SharedPlugin;
use bevy::prelude::*;
use lightyear::prelude::{Deserialize, Serialize};
use lightyear_examples_common::app::{Apps, Cli, Mode};
use lightyear_examples_common::settings::ServerTransports;

#[cfg(feature = "client")]
mod client;
mod protocol;

#[cfg(feature = "gui")]
mod renderer;
mod server;
mod settings;
mod shared;

pub const HOST_SERVER_PORT: u16 = 5050;

fn main() {
    let mut cli = Cli::default();
    let mut settings = get_settings();
    let mut is_dedicated_server = true;

    // in this example, every client will actually launch in host-server mode
    // the reason is that we want every client to be able to be the 'host' of a lobby
    // so every client needs to have the ServerPlugins included in the app
    match cli.mode {
        Some(Mode::Client { client_id }) => {
            is_dedicated_server = false;
            cli.mode = Some(Mode::HostServer { client_id });
            // when the client acts as host, we will use port UDP:5050 for the transport
            settings.server.transport = vec![ServerTransports::Udp {
                local_port: HOST_SERVER_PORT,
            }];
        }
        _ => {}
    }

    // build the bevy app (this adds common plugins such as the DefaultPlugins)
    // and returns the `ClientConfig` and `ServerConfig` so that we can modify them
    let mut apps = Apps::new(settings.clone(), cli, env!("CARGO_PKG_NAME").to_string());
    // we do not modify the configurations of the plugins, so we can just build
    // the `ClientPlugins` and `ServerPlugins` plugin groups
    apps.add_lightyear_plugins();
    apps.add_user_shared_plugin(SharedPlugin);
    #[cfg(feature = "client")]
    apps.add_user_client_plugin(client::ExampleClientPlugin { settings });
    apps.add_user_server_plugin(server::ExampleServerPlugin {
        is_dedicated_server,
    });
    #[cfg(feature = "gui")]
    apps.add_user_renderer_plugin(renderer::ExampleRendererPlugin);

    // run the app
    apps.run();
}
