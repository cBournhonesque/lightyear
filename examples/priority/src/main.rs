#![allow(unused_imports)]
#![allow(unused_variables)]
#![allow(dead_code)]
use crate::client::ExampleClientPlugin;
use crate::server::ExampleServerPlugin;
use crate::shared::SharedPlugin;
use bevy::prelude::*;
use lightyear::prelude::server::PacketConfig;
use lightyear_examples_common::app::{Apps, Cli};
use lightyear_examples_common::settings::{read_settings, Settings};

mod client;
mod protocol;
mod server;
mod shared;

fn main() {
    let cli = Cli::default();
    let settings_str = include_str!("../assets/settings.ron");
    let settings = read_settings::<Settings>(settings_str);
    // build the bevy app (this adds common plugin such as the DefaultPlugins)
    let mut apps = Apps::new(settings, cli, env!("CARGO_PKG_NAME").to_string());
    // for this example, we will use input delay and a correction function
    apps.update_lightyear_server_config(|config| {
        // for this example, we will put a bandwidth cap on the server-side
        config.packet = PacketConfig::default()
            .enable_bandwidth_cap()
            // we can set the max bandwidth to 56 KB/s
            .with_send_bandwidth_bytes_per_second_cap(3000);
    })
    // add the `ClientPlugins` and `ServerPlugins` plugin groups
    .add_lightyear_plugins()
    // add our plugins
    .add_user_plugins(ExampleClientPlugin, ExampleServerPlugin, SharedPlugin);
    // run the app
    apps.run();
}
