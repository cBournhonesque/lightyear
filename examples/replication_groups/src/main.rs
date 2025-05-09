#![allow(unused_imports)]
#![allow(unused_variables)]
#![allow(dead_code)]
extern crate alloc;

use crate::settings::get_settings;
use bevy::prelude::*;
use lightyear_examples_common::app::{Apps, Cli};
use lightyear_examples_common::settings::Settings;

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
    let settings = get_settings();
    // build the bevy app (this adds common plugin such as the DefaultPlugins)
    // and returns the `ClientConfig` and `ServerConfig` so that we can modify them if needed
    let mut apps = Apps::new(settings, cli, env!("CARGO_PKG_NAME").to_string());

    apps.add_lightyear_plugins();
    apps.add_user_shared_plugin(shared::SharedPlugin);
    #[cfg(feature = "client")]
    apps.add_user_client_plugin(client::ExampleClientPlugin);
    #[cfg(feature = "server")]
    apps.add_user_server_plugin(server::ExampleServerPlugin);
    #[cfg(feature = "gui")]
    apps.add_user_renderer_plugin(renderer::ExampleRendererPlugin);

    // run the app
    apps.run();
}
