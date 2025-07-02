//! This example showcases how to use Lightyear with Bevy, to easily get replication along with prediction/interpolation working.
//!
//! There is a lot of setup code, but it's mostly to have the examples work in all possible configurations of transport.
//! (all transports are supported, as well as running the example in client-and-server or host-server mode)
//!
//!
//! Run with
//! - `cargo run -- server`
//! - `cargo run -- client -c 1`
#![allow(clippy::all)]
#![allow(unused_imports)]
#![allow(unused_variables)]
#![allow(dead_code)]
use bevy::prelude::*;
use core::time::Duration;
use lightyear::prelude::{DeltaManager, Server};
use lightyear_examples_common::cli::{Cli, Mode};
use lightyear_examples_common::shared::FIXED_TIMESTEP_HZ;
use protocol::ProtocolPlugin;

#[cfg(feature = "client")]
use crate::client::ExampleClientPlugin;
#[cfg(feature = "server")]
use crate::server::ExampleServerPlugin;

#[cfg(feature = "client")]
mod client;
mod protocol;
#[cfg(feature = "gui")]
mod renderer;
#[cfg(feature = "server")]
mod server;

mod shared;

fn main() {
    let cli = Cli::default();

    let mut app = cli.build_app(Duration::from_secs_f64(1.0 / FIXED_TIMESTEP_HZ), true);

    app.add_plugins(ProtocolPlugin);
    cli.spawn_connections(&mut app);
    match cli.mode {
        #[cfg(feature = "client")]
        Some(Mode::Client { .. }) => {
            app.add_plugins(ExampleClientPlugin);
        }
        #[cfg(feature = "server")]
        Some(Mode::Server) => {
            app.add_plugins(ExampleServerPlugin);
            add_delta_manager(&mut app);
        }
        #[cfg(all(feature = "client", feature = "server"))]
        Some(Mode::HostClient { client_id }) => {
            app.add_plugins(ExampleClientPlugin);
            app.add_plugins(ExampleServerPlugin);
            add_delta_manager(&mut app);
        }
        _ => {}
    }
    #[cfg(feature = "gui")]
    app.add_plugins(renderer::ExampleRendererPlugin);

    // run the app
    app.run();
}

/// To enable delta compression, we need to add the DeltaManager component on the server.
#[cfg(feature = "server")]
fn add_delta_manager(app: &mut App) {
    let server = app
        .world_mut()
        .query_filtered::<Entity, With<Server>>()
        .single(app.world_mut())
        .unwrap();

    // set some input-delay since we are predicting all entities
    app.world_mut()
        .entity_mut(server)
        .insert(DeltaManager::default());
}
