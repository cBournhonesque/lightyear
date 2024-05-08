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
