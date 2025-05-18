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
use lightyear_examples_common::cli::{Cli, Mode};
use lightyear_examples_common::shared::{
    CLIENT_PORT, FIXED_TIMESTEP_HZ, SERVER_ADDR, SERVER_PORT, SHARED_SETTINGS,
};
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

// mod settings; // Settings are now handled by common_new
mod shared;

fn main() {
    let cli = Cli::default();

    // #[cfg(target_family = "wasm")]
    // lightyear_examples_common::settings::modify_digest_on_wasm(&mut settings.client); // Assuming this is handled by common_new now

    let mut app = cli.build_app(
        Duration::from_secs_f64(1.0 / FIXED_TIMESTEP_HZ),
        false, // No physics loop needed
    );

    #[cfg(feature = "client")]
    {
        app.add_plugins(ExampleClientPlugin);
        if matches!(cli.mode, Some(Mode::Client { .. })) {
            use lightyear::prelude::Connect;
            use lightyear_examples_common::client::{ClientTransports, ExampleClient};
            let client = app
                .world_mut()
                .spawn(ExampleClient {
                    client_id: cli
                        .client_id()
                        .expect("You need to specify a client_id via `-c ID`"),
                    client_port: CLIENT_PORT,
                    server_addr: SERVER_ADDR,
                    conditioner: None,
                    transport: ClientTransports::Udp, // Assuming UDP
                    shared: SHARED_SETTINGS,
                })
                .id();
            app.world_mut().trigger_targets(Connect, client)
        }
    }

    #[cfg(feature = "server")]
    {
        use lightyear::connection::server::Start;
        use lightyear_examples_common::server::{ExampleServer, ServerTransports};

        app.add_plugins(ExampleServerPlugin);
        if matches!(cli.mode, Some(Mode::Server)) {
            let server = app
                .world_mut()
                .spawn(ExampleServer {
                    conditioner: None,
                    transport: ServerTransports::Udp {
                        // Assuming UDP
                        local_port: SERVER_PORT,
                    },
                    shared: SHARED_SETTINGS,
                })
                .id();
            app.world_mut().trigger_targets(Start, server);
        }
    }

    // add the protocol plugin
    app.add_plugins(ProtocolPlugin);

    #[cfg(feature = "gui")]
    app.add_plugins(renderer::ExampleRendererPlugin);

    // run the app
    app.run();
}
