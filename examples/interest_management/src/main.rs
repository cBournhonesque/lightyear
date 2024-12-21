#![allow(unused_imports)]
#![allow(unused_variables)]
#![allow(dead_code)]
use crate::shared::SharedPlugin;
use bevy::prelude::*;
use lightyear_examples_common::app::Apps;
use lightyear_examples_common::settings::{read_settings, Settings};

#[cfg(feature = "client")]
mod client;
mod protocol;

#[cfg(feature = "gui")]
mod renderer;
#[cfg(feature = "server")]
mod server;
mod shared;

fn main() {
    let cli = lightyear_examples_common::app::cli();
    let settings_str = include_str!("../assets/settings.ron");
    let settings = read_settings::<Settings>(settings_str);
    // build the bevy app (this adds common plugin such as the DefaultPlugins)
    let mut apps = Apps::new(settings, cli, env!("CARGO_PKG_NAME").to_string());
    // add `ClientPlugins` and `ServerPlugins` plugin groups
    apps.add_lightyear_plugins();
    #[cfg(feature = "client")]
    apps.add_user_client_plugin(client::ExampleClientPlugin);
    #[cfg(feature = "server")]
    apps.add_user_server_plugin(server::ExampleServerPlugin);
    #[cfg(feature = "gui")]
    apps.add_user_renderer_plugin(renderer::ExampleRendererPlugin);
    // run the app
    apps.run();
}
