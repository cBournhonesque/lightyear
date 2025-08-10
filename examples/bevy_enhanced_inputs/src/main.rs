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

#[cfg(feature = "client")]
use crate::client::ExampleClientPlugin;
#[cfg(feature = "server")]
use crate::server::ExampleServerPlugin;
use crate::shared::SharedPlugin;
use bevy::prelude::*;
use core::time::Duration;
use lightyear::prelude::{ReplicationSender, SendUpdatesMode};
use lightyear_examples_common::cli::{Cli, Mode};

use lightyear_examples_common::shared::{FIXED_TIMESTEP_HZ, SEND_INTERVAL};

#[cfg(feature = "client")]
mod client;
mod protocol;
#[cfg(feature = "gui")]
mod renderer;
#[cfg(feature = "server")]
mod server;

mod shared;

/// When running the example as a binary, we only support Client or Server mode.
fn main() {
    let cli = Cli::default();

    let mut app = cli.build_app(Duration::from_secs_f64(1.0 / FIXED_TIMESTEP_HZ), true);

    app.add_plugins(SharedPlugin);

    cli.spawn_connections(&mut app);

    match cli.mode {
        #[cfg(feature = "client")]
        Some(Mode::Client { .. }) => {
            app.add_plugins(ExampleClientPlugin);
            add_input_delay(&mut app);
        }
        #[cfg(feature = "server")]
        Some(Mode::Server) => {
            app.add_plugins(ExampleServerPlugin);
        }
        #[cfg(all(feature = "client", feature = "server"))]
        Some(Mode::HostClient { client_id }) => {
            app.add_plugins(ExampleClientPlugin);
            app.add_plugins(ExampleServerPlugin);
            add_input_delay(&mut app);
        }
        _ => {}
    }

    #[cfg(feature = "gui")]
    app.add_plugins(renderer::ExampleRendererPlugin);

    app.run();
}

#[cfg(feature = "client")]
fn add_input_delay(app: &mut App) {
    use lightyear::prelude::client::{Input, InputDelayConfig};
    use lightyear::prelude::{Client, InputTimeline, Timeline};
    let client = app
        .world_mut()
        .query_filtered::<Entity, With<Client>>()
        .single(app.world_mut())
        .unwrap();

    // we need to add a ReplicationSender to the client entity to replicate the Action entities to the server
    app.world_mut()
        .entity_mut(client)
        .insert(ReplicationSender::new(
            SEND_INTERVAL,
            SendUpdatesMode::SinceLastAck,
            false,
        ));

    // set some input-delay since we are predicting all entities
    app.world_mut()
        .entity_mut(client)
        .insert(InputTimeline(Timeline::from(
            Input::default().with_input_delay(InputDelayConfig::fixed_input_delay(10)),
        )));
}
