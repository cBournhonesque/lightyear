//! This example showcases how to send a ConnectToken securely from a game server to the client.
//!
//! Lightyear requires the client to have a ConnectToken to connect to the server. Normally the client
//! would get it from a backend server (for example via a HTTPS connection to a webserver).
//! If you don't have a separated backend server, you can use the game server to generate the ConnectToken.
//!
//! Run with
//! - `cargo run -- server`
//! - `cargo run -- client -c 1`
#![allow(unused_imports)]
#![allow(unused_variables)]
#![allow(dead_code)]

use bevy::prelude::*;
use core::time::Duration;
use lightyear_examples_common_new::cli::{Cli, Mode};
use lightyear_examples_common_new::shared::{CLIENT_PORT, FIXED_TIMESTEP_HZ, SERVER_ADDR, SERVER_PORT, SHARED_SETTINGS};

#[cfg(feature = "client")]
use crate::client::ExampleClientPlugin;
use crate::protocol::ProtocolPlugin;
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

    #[cfg(target_family = "wasm")]
    lightyear_examples_common::settings::modify_digest_on_wasm(&mut settings.client); // Assuming this might still be needed for wasm

    let mut app = cli.build_app(
        Duration::from_secs_f64(1.0 / FIXED_TIMESTEP_HZ),
        true
    );

    app.add_plugins(ProtocolPlugin);

    // NOTE: The auth-specific parameters (addresses, keys) previously passed to plugins
    // are not included here. This might require adjustments in the client/server plugins
    // or the ExampleClient/ExampleServer entities if they need this data.

    #[cfg(feature = "client")]
    {
        app.add_plugins(ExampleClientPlugin); // Assuming ExampleClientPlugin doesn't need args now
        if matches!(cli.mode, Some(Mode::Client { .. })) {
            use lightyear::prelude::Connect;
            use lightyear_examples_common_new::client::{ClientTransports, ExampleClient};
            let client = app.world_mut().spawn(ExampleClient {
                client_id: cli.client_id().expect("You need to specify a client_id via `-c ID`"),
                client_port: CLIENT_PORT,
                server_addr: SERVER_ADDR, // This is the game server addr, auth might be different
                conditioner: None,
                transport: ClientTransports::Udp, // Auth example likely uses UDP
                shared: SHARED_SETTINGS,
            }).id();
            app.world_mut().trigger_targets(Connect, client)
        }
    }

    #[cfg(feature = "server")]
    {
        use lightyear_examples_common_new::server::{ExampleServer, ServerTransports};
        use lightyear::connection::server::Start;

        app.add_plugins(ExampleServerPlugin); // Assuming ExampleServerPlugin doesn't need args now
        if matches!(cli.mode, Some(Mode::Server)) {
            let server = app.world_mut().spawn(ExampleServer {
                conditioner: None,
                transport: ServerTransports::Udp { // Auth example likely uses UDP
                    local_port: SERVER_PORT
                },
                shared: SHARED_SETTINGS // Contains protocol_id, private_key
            }).id();
            app.world_mut().trigger_targets(Start, server);
        }
    }

    #[cfg(feature = "gui")]
    app.add_plugins(renderer::ExampleRendererPlugin);

    app.run();
}
