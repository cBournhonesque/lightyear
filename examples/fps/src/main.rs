#![allow(clippy::all)]
#![allow(unused_imports)]
#![allow(unused_variables)]
#![allow(dead_code)]
use bevy::prelude::*;
use core::time::Duration;
use lightyear_examples_common::cli::{Cli, Mode};
use lightyear_examples_common::shared::FIXED_TIMESTEP_HZ;

#[cfg(feature = "client")]
use crate::client::ExampleClientPlugin;
#[cfg(feature = "p2p")]
use crate::p2p::ExampleP2PPlugin;
#[cfg(feature = "server")]
use crate::server::ExampleServerPlugin;
use crate::shared::SharedPlugin;

mod automation;
#[cfg(feature = "client")]
mod client;
mod debug;
#[cfg(feature = "p2p")]
mod p2p;
mod protocol;

#[cfg(feature = "gui")]
mod renderer;

#[cfg(feature = "server")]
mod server;
mod shared;

fn main() {
    let cli = Cli::default();
    let headless = cli.headless();

    let mut app = cli.build_app(Duration::from_secs_f64(1.0 / FIXED_TIMESTEP_HZ), true);

    app.add_plugins(SharedPlugin);

    cli.spawn_connections(&mut app);

    match cli.mode {
        #[cfg(all(feature = "client", any(not(feature = "p2p"), feature = "netcode")))]
        Some(Mode::Client { .. }) => {
            app.add_plugins(ExampleClientPlugin);
        }
        #[cfg(feature = "p2p")]
        Some(Mode::P2P { .. }) => {
            app.add_plugins((ExampleClientPlugin, ExampleP2PPlugin));
        }
        #[cfg(feature = "server")]
        Some(Mode::Server) => {
            app.add_plugins(ExampleServerPlugin);
        }
        #[cfg(all(feature = "client", feature = "server"))]
        Some(Mode::HostClient { client_id }) => {
            app.add_plugins(ExampleClientPlugin);
            app.add_plugins(ExampleServerPlugin);
        }
        _ => {}
    }

    #[cfg(feature = "gui")]
    if !headless {
        app.add_plugins(renderer::ExampleRendererPlugin);
    }

    // run the app
    app.run();
}
