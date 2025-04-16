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

use bevy::prelude::*;
use core::time::Duration;

#[cfg(feature = "client")]
mod client;
mod protocol;
#[cfg(feature = "gui")]
mod renderer;
#[cfg(feature = "server")]
mod server;

pub const FIXED_TIMESTEP_HZ: f64 = 64.0;

fn main() {
    // let cli = Cli::default();
    // #[allow(unused_mut)]
    // let mut settings = get_settings();

    #[cfg(target_family = "wasm")]
    lightyear_examples_common::settings::modify_digest_on_wasm(&mut settings.client);


    // build the bevy app (this adds common plugin such as the DefaultPlugins)
    // and returns the `ClientConfig` and `ServerConfig` so that we can modify them if needed
    // let mut apps = Apps::new(settings, cli, env!("CARGO_PKG_NAME").to_string());
    // add the `ClientPlugins` and `ServerPlugins` plugin groups
    let mut app = App::new();
    app.add_plugins(DefaultPlugins);
    #[cfg(feature = "client")]
    app.add_plugins(lightyear_new::prelude::client::ClientPlugins {
        tick_duration: Duration::from_secs_f64(1.0 / FIXED_TIMESTEP_HZ),
    });

    // apps.add_lightyear_plugins();
    // apps.add_user_shared_plugin(ProtocolPlugin);
    #[cfg(feature = "server")]
    app.add_plugins(lightyear_new::prelude::server::ServerPlugins {
        tick_duration: Duration::from_secs_f64(1.0 / FIXED_TIMESTEP_HZ),
    });
    #[cfg(feature = "gui")]
    apps.add_user_renderer_plugin(renderer::ExampleRendererPlugin);
    // run the app
    app.run();
}
