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
#![allow(unused_mut)]
#![allow(unused_variables)]
#![allow(dead_code)]

use bevy::prelude::*;
use core::time::Duration;
use lightyear::prelude::{LinkConditionerConfig, RecvLinkConditioner};
use lightyear_examples_common::cli::{Cli, Mode};
use lightyear_examples_common::shared::{
    CLIENT_PORT, FIXED_TIMESTEP_HZ, SERVER_ADDR, SERVER_PORT, SHARED_SETTINGS,
};

#[cfg(feature = "client")]
use crate::client::ExampleClientPlugin;
use crate::shared::SharedPlugin;

#[cfg(feature = "client")]
mod client;
mod protocol;

#[cfg(feature = "gui")]
mod renderer;
mod server;
mod shared;

pub const HOST_SERVER_PORT: u16 = 5050;

fn main() {
    let cli = Cli::default();

    let tick_duration = Duration::from_secs_f64(1.0 / FIXED_TIMESTEP_HZ);
    let mut app = cli.build_app(tick_duration, true);

    app.add_plugins(SharedPlugin);

    let mut is_dedicated_server = true;

    // in this example, every client will actually launch in host-server mode
    // the reason is that we want every client to be able to be the 'host' of a lobby
    // so every client needs to have the ServerPlugins included in the app
    match cli.mode {
        #[cfg(feature = "client")]
        Some(Mode::Client { client_id }) => {
            // we want every client to be able to act as host-server so we
            // add the server plugins
            app.add_plugins(lightyear::prelude::server::ServerPlugins { tick_duration });
            is_dedicated_server = false;
        }
        _ => {}
    }

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
                    conditioner: Some(RecvLinkConditioner::new(
                        LinkConditionerConfig::average_condition(),
                    )),
                    transport: ClientTransports::WebTransport,
                    shared: SHARED_SETTINGS,
                })
                .id();
            app.world_mut().trigger_targets(Connect, client)
        }
    }

    {
        use lightyear::connection::server::Start;
        use lightyear_examples_common::server::WebTransportCertificateSettings;
        use lightyear_examples_common::server::{ExampleServer, ServerTransports};

        app.add_plugins(server::ExampleServerPlugin {
            is_dedicated_server,
        });
        if matches!(cli.mode, Some(Mode::Server)) {
            let server = app
                .world_mut()
                .spawn(ExampleServer {
                    conditioner: None,
                    transport: ServerTransports::WebTransport {
                        local_port: SERVER_PORT,
                        certificate: WebTransportCertificateSettings::FromFile {
                            cert: "../certificates/cert.pem".to_string(),
                            key: "../certificates/key.pem".to_string(),
                        },
                    },
                    shared: SHARED_SETTINGS,
                })
                .id();
            app.world_mut().trigger_targets(Start, server);
        }
    }

    #[cfg(feature = "gui")]
    app.add_plugins(renderer::ExampleRendererPlugin);

    app.run();
}
