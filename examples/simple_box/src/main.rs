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
#[cfg(feature = "client")]
use crate::client::ExampleClientPlugin;
#[cfg(feature = "gui")]
use crate::renderer::ExampleRendererPlugin;
#[cfg(feature = "server")]
use crate::server::ExampleServerPlugin;
use bevy::prelude::*;
use lightyear_examples_common::app::{Apps, Cli};
use lightyear_examples_common::settings::{read_settings, Settings};
use protocol::ProtocolPlugin;

#[cfg(feature = "client")]
mod client;
mod protocol;
#[cfg(feature = "gui")]
mod renderer;
#[cfg(feature = "server")]
mod server;
mod shared;

fn main() {
    let cli = Cli::default();
    let settings_str = include_str!("../assets/settings.ron");
    #[allow(unused_mut)]
    let mut settings = read_settings::<Settings>(settings_str);
    #[cfg(target_family = "wasm")]
    lightyear_examples_common::settings::modify_digest_on_wasm(&mut settings.client);
    // build the bevy app (this adds common plugin such as the DefaultPlugins)
    // and returns the `ClientConfig` and `ServerConfig` so that we can modify them if needed
    let mut apps = Apps::new(settings, cli, env!("CARGO_PKG_NAME").to_string());
    // add the `ClientPlugins` and `ServerPlugins` plugin groups
    apps.add_lightyear_plugins();
    apps.add_user_shared_plugin(ProtocolPlugin);
    #[cfg(feature = "client")]
    apps.add_user_client_plugin(ExampleClientPlugin);
    #[cfg(feature = "server")]
    apps.add_user_server_plugin(ExampleServerPlugin);
    #[cfg(feature = "gui")]
    apps.add_user_renderer_plugin(ExampleRendererPlugin);
    // run the app
    apps.run();
}
