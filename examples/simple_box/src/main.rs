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

mod client;
mod protocol;
mod server;
mod shared;

fn main() {
    let cli = common::app::cli();
    let settings_str = include_str!("../assets/settings.ron");
    let settings = common::settings::settings::<Settings>(settings_str);
    // build the bevy app (this adds common plugin such as the DefaultPlugins)
    // and returns the `ClientConfig` and `ServerConfig` so that we can modify them
    let mut app = common::app::build_app(settings, cli);
    // we do not modify the configurations of the plugins, so we can just build
    // the `ClientPlugins` and `ServerPlugins` plugin groups
    app.add_plugin_groups();
    // add our plugins
    match &mut app {
        Apps::Client { app, .. } => {
            app.add_plugins((ExampleClientPlugin, SharedPlugin));
        }
        Apps::Server { app, .. } => {
            app.add_plugins((ExampleServerPlugin, SharedPlugin));
        }
        Apps::ListenServer {
            client_app,
            server_app,
            ..
        } => {
            client_app.add_plugins((ExampleClientPlugin, SharedPlugin));
            server_app.add_plugins((ExampleServerPlugin, SharedPlugin));
        }
        Apps::HostServer { app, .. } => {
            app.add_plugins((ExampleClientPlugin, ExampleServerPlugin, SharedPlugin));
        }
    }
    // run the app
    app.run();
}
