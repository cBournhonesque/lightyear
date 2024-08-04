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
use lightyear::prelude::client::PacketConfig;
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
    // and returns the `ClientConfig` and `ServerConfig` so that we can modify them if needed
    let mut apps = Apps::new(settings, cli);
    apps.update_lightyear_client_config(|config| {
        // for this example, we will put a bandwidth cap on the server-side
        config.packet = PacketConfig::default()
            .enable_bandwidth_cap()
            // we can set the max bandwidth to 256 KB/s
            .with_send_bandwidth_bytes_per_second_cap(256000);
    });
    // add the `ClientPlugins` and `ServerPlugins` plugin groups
    apps.add_lightyear_plugins()
        // add our plugins
        .add_user_plugins(ExampleClientPlugin, ExampleServerPlugin, SharedPlugin);
    // run the app
    apps.run();
}
