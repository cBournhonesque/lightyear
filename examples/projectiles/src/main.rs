#![allow(clippy::all)]
#![allow(unused_imports)]
#![allow(unused_variables)]
#![allow(dead_code)]
extern crate core;

use bevy::prelude::*;
use core::time::Duration;
use lightyear::prelude::{ReplicationSender, SendUpdatesMode};
use lightyear_examples_common::cli::{Cli, Mode};
use lightyear_examples_common::shared::{FIXED_TIMESTEP_HZ, SEND_INTERVAL};

#[cfg(feature = "client")]
use crate::client::ExampleClientPlugin;
use crate::protocol::ClientContext;
#[cfg(feature = "server")]
use crate::server::ExampleServerPlugin;
use crate::shared::SharedPlugin;

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

    app.add_plugins(SharedPlugin);

    cli.spawn_connections(&mut app);

    match cli.mode {
        #[cfg(feature = "client")]
        Some(Mode::Client { .. }) => {
            app.add_plugins(ExampleClientPlugin);
            update_client(&mut app);
        }
        #[cfg(feature = "server")]
        Some(Mode::Server) => {
            app.add_plugins(ExampleServerPlugin);
        }
        #[cfg(all(feature = "client", feature = "server"))]
        Some(Mode::HostClient { client_id }) => {
            app.add_plugins(ExampleClientPlugin);
            app.add_plugins(ExampleServerPlugin);
            update_client(&mut app);
        }
        _ => {}
    }

    #[cfg(feature = "gui")]
    app.add_plugins(renderer::ExampleRendererPlugin);

    // run the app
    app.run();
}

#[cfg(feature = "client")]
fn update_client(app: &mut App) {
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
        .insert((
            ReplicationSender::new(
                SEND_INTERVAL,
                SendUpdatesMode::SinceLastAck,
                false,
            ),
        ));

    // // set some input-delay since we are predicting all entities
    // app.world_mut()
    //     .entity_mut(client)
    //     .insert(InputTimeline(Timeline::from(
    //         Input::default().with_input_delay(InputDelayConfig::fixed_input_delay(0)),
    //     )));
}