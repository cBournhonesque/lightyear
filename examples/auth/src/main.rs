//! This example showcases how to send a ConnectToken securely from a game server to the client.
//!
//! Lightyear requires the client to have a ConnectToken to connect to the server. Normally the client
//! would get it from a backend server (for example via a HTTPS connection to a webserver).
//! If you don't have a separated backend server, you can use the game server to generate the ConnectToken.
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
#[cfg(feature = "client")]
use lightyear::netcode::NetcodeClient;
use lightyear_examples_common::cli::{Cli, Mode};
use lightyear_examples_common::shared::{
    CLIENT_PORT, FIXED_TIMESTEP_HZ, SERVER_ADDR, SERVER_PORT, SHARED_SETTINGS,
};

#[cfg(feature = "client")]
use crate::client::ExampleClientPlugin;
#[cfg(feature = "server")]
use crate::server::ExampleServerPlugin;
use crate::shared::auth_backend_address;
use lightyear::connection::server::Start;
use lightyear::prelude::ComponentRegistry;
#[cfg(feature = "client")]
use lightyear_examples_common::client::{ClientTransports, ExampleClient};
#[cfg(feature = "server")]
use lightyear_examples_common::server::{ExampleServer, ServerTransports};

#[cfg(feature = "client")]
mod client;
#[cfg(feature = "server")]
mod server;
// mod settings; // Settings are now handled by common_new
mod shared;

fn main() {
    let cli = Cli::default();
    let auth_backend_address = auth_backend_address();

    let mut app = cli.build_app(Duration::from_secs_f64(1.0 / FIXED_TIMESTEP_HZ), true);
    // This example does not register any replicated protocol components, but the shared example
    // harness still enables prediction/replication features on the client. Seed the registry so
    // PredictionPlugin::finish has the expected resource even when the example itself has no
    // protocol setup.
    app.init_resource::<ComponentRegistry>();

    match cli.mode {
        None => {}
        #[cfg(feature = "client")]
        Some(Mode::Client { client_id }) => {
            app.add_plugins(ExampleClientPlugin {
                auth_backend_address,
            });
            let client = app
                .world_mut()
                .spawn(ExampleClient {
                    client_id: cli.client_id().unwrap_or(0),
                    client_port: CLIENT_PORT,
                    server_addr: SERVER_ADDR,
                    conditioner: None,
                    transport: ClientTransports::Udp,
                    shared: SHARED_SETTINGS,
                })
                .id();
            assert!(
                app.world().get::<NetcodeClient>(client).is_some(),
                "The example only works with netcode enabled!"
            );
            // remove the NetcodeClient for the example, as we want to show how we can
            // send the ConnectToken from the server to the client to build a NetcodeClient
            app.world_mut().entity_mut(client).remove::<NetcodeClient>();
        }
        #[cfg(feature = "server")]
        Some(Mode::Server) => {
            app.add_plugins(ExampleServerPlugin {
                game_server_addr: SERVER_ADDR,
                auth_backend_addr: auth_backend_address,
            });
            let server = app
                .world_mut()
                .spawn(ExampleServer {
                    conditioner: None,
                    transport: ServerTransports::Udp {
                        local_port: SERVER_PORT,
                    },
                    shared: SHARED_SETTINGS,
                })
                .id();
            app.world_mut().trigger(Start { entity: server });
        }
        #[cfg(all(feature = "client", feature = "server"))]
        Some(Mode::HostClient { client_id }) => {
            app.add_plugins(ExampleClientPlugin {
                auth_backend_address,
            });
            app.add_plugins(ExampleServerPlugin {
                game_server_addr: SERVER_ADDR,
                auth_backend_addr: auth_backend_address,
            });
            let server = app
                .world_mut()
                .spawn(ExampleServer {
                    conditioner: None,
                    transport: ServerTransports::Udp {
                        local_port: SERVER_PORT,
                    },
                    shared: SHARED_SETTINGS,
                })
                .id();
            app.world_mut().trigger(Start { entity: server });
            let client = app
                .world_mut()
                .spawn(ExampleClient {
                    client_id: cli.client_id().unwrap_or(0),
                    client_port: CLIENT_PORT,
                    server_addr: SERVER_ADDR,
                    conditioner: None,
                    transport: ClientTransports::Udp,
                    shared: SHARED_SETTINGS,
                })
                .id();
            app.world_mut().entity_mut(client).remove::<NetcodeClient>();
        }
        _ => {}
    }

    app.run();
}
