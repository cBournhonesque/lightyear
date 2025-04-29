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
use bevy::prelude::*;
use lightyear_examples_common::app::{Apps, Cli};
use lightyear_examples_common::settings::Settings;
use protocol::ProtocolPlugin;

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
    #[allow(unused_mut)]
    let mut settings = get_settings();
    #[cfg(target_family = "wasm")]
    lightyear_examples_common::settings::modify_digest_on_wasm(&mut settings.client);
    // build the bevy app (this adds common plugin such as the DefaultPlugins)
    // and returns the `ClientConfig` and `ServerConfig` so that we can modify them if needed
    let mut apps = Apps::new(settings.clone(), cli, env!("CARGO_PKG_NAME").to_string());
    // add the `ClientPlugins` and `ServerPlugins` plugin groups
    apps.add_lightyear_plugins();
    apps.add_user_shared_plugin(ProtocolPlugin);
    #[cfg(feature = "client")]
    apps.add_user_client_plugin(client::ExampleClientPlugin);
    #[cfg(feature = "server")]
    apps.add_user_server_plugin(server::ExampleServerPlugin);
    #[cfg(feature = "gui")]
    apps.add_user_renderer_plugin(renderer::ExampleRendererPlugin);
    // run the app
    apps.run();
}
