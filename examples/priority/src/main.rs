#![allow(unused_imports)]
#![allow(unused_variables)]
#![allow(dead_code)]
use crate::client::ExampleClientPlugin;
use crate::server::ExampleServerPlugin;
use crate::shared::SharedPlugin;
use bevy::prelude::*;
use common::app::Apps;
use common::settings::Settings;
use lightyear::prelude::server::PacketConfig;

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

    // for this example, we will put a bandwidth cap on the server-side
    let packet_config = PacketConfig::default()
        // by default there is no bandwidth limit so we need to enable it
        .enable_bandwidth_cap()
        // we can set the max bandwidth to 56 KB/s
        .with_send_bandwidth_bytes_per_second_cap(1500);
    match &mut app {
        Apps::Server { config, .. } => {
            config.packet = packet_config;
        }
        Apps::ListenServer { server_config, .. } => {
            server_config.packet = packet_config;
        }
        Apps::HostServer { server_config, .. } => {
            server_config.packet = packet_config;
        }
        _ => {}
    }

    // add the `ClientPlugins` and `ServerPlugins` plugin groups
    app.add_lightyear_plugin_groups();
    // add our plugins
    app.add_plugins(ExampleClientPlugin, ExampleServerPlugin, SharedPlugin);
    // run the app
    app.run();
}
