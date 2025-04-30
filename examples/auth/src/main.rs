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
use lightyear::netcode::NetcodeClient;
use lightyear_examples_common_new::cli::{Cli, Mode};
use lightyear_examples_common_new::shared::{CLIENT_PORT, FIXED_TIMESTEP_HZ, SERVER_ADDR, SERVER_PORT, SHARED_SETTINGS};

use crate::client::ExampleClientPlugin;
use crate::server::ExampleServerPlugin;
use crate::shared::AUTH_BACKEND_ADDRESS;
use lightyear::connection::server::Start;
use lightyear_examples_common_new::client::{ClientTransports, ExampleClient};
use lightyear_examples_common_new::server::{ExampleServer, ServerTransports};

mod client;

mod server;
// mod settings; // Settings are now handled by common_new
mod shared;



fn main() {
    let cli = Cli::default();

    let mut app = cli.build_app(
        Duration::from_secs_f64(1.0 / FIXED_TIMESTEP_HZ),
        true
    );

    match cli.mode {
        None => {}
        Some(Mode::Client { client_id }) => {
            app.add_plugins(ExampleClientPlugin {
                auth_backend_address: AUTH_BACKEND_ADDRESS
            });
            let client = app.world_mut().spawn(ExampleClient {
                client_id: cli.client_id().unwrap_or(0),
                client_port: CLIENT_PORT,
                server_addr: SERVER_ADDR,
                conditioner: None,
                transport: ClientTransports::Udp,
                shared: SHARED_SETTINGS,
            }).id();
            assert!(app.world().get::<NetcodeClient>(client).is_some(), "The example only works with netcode enabled!");
            // remove the NetcodeClient for the example, as we want to show how we can
            // send the ConnectToken from the server to the client to build a NetcodeClient
            app.world_mut().entity_mut(client).remove::<NetcodeClient>();
        }
        Some(Mode::Server) => {
            app.add_plugins(ExampleServerPlugin {
                game_server_addr: SERVER_ADDR,
                auth_backend_addr: AUTH_BACKEND_ADDRESS,
            });
            let server = app.world_mut().spawn(ExampleServer {
                conditioner: None,
                transport: ServerTransports::Udp {
                    local_port: SERVER_PORT
                },
                shared: SHARED_SETTINGS
            }).id();
            app.world_mut().trigger_targets(Start, server);
        }
        _ => {}
    }


    app.run();
}
