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
    // and returns the `ClientConfig` and `ServerConfig` so that we can modify them if needed
    let mut app = common::app::build_app(settings, cli);
    // add the `ClientPlugins` and `ServerPlugins` plugin groups
    app.add_lightyear_plugin_groups();
    // add our plugins
    app.add_plugins(ExampleClientPlugin, ExampleServerPlugin, SharedPlugin);
    // run the app
    app.run();
}
