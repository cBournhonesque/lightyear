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
use crate::protocol::ProtocolPlugin;
#[cfg(feature = "server")]
use crate::server::ExampleServerPlugin;
use bevy::prelude::*;
use core::net::{IpAddr, Ipv4Addr, SocketAddr};
use core::time::Duration;
use lightyear_examples_common_new::cli::{Cli, Mode};
use lightyear_examples_common_new::shared::SharedSettings;

#[cfg(feature = "client")]
mod client;
mod protocol;
#[cfg(feature = "gui")]
mod renderer;
#[cfg(feature = "server")]
mod server;

mod shared;

pub const FIXED_TIMESTEP_HZ: f64 = 64.0;
pub const SERVER_PORT: u16 = 5000;
/// 0 means that the OS will assign any available port
pub const CLIENT_PORT: u16 = 0;
pub const SERVER_ADDR: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), SERVER_PORT);
pub const SHARED_SETTINGS: SharedSettings = SharedSettings {
    protocol_id: 0,
    private_key: [
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0,
    ],
};

/// When running the example as a binary, we only support Client or Server mode.
fn main() {
    let cli = Cli::default();

    #[cfg(target_family = "wasm")]
    lightyear_examples_common::settings::modify_digest_on_wasm(&mut settings.client);

    let mut app = cli.build_app(
        Duration::from_secs_f64(1.0 / FIXED_TIMESTEP_HZ),
        true
    );

    app.add_plugins(ProtocolPlugin);

    #[cfg(feature = "client")]
    {
        use lightyear::connection::client::Connect;

        app.add_plugins(ExampleClientPlugin);
        if matches!(cli.mode, Some(Mode::Client { .. })) {
            use lightyear::prelude::client::Connect;
            use lightyear_examples_common_new::client::{ClientTransports, ExampleClient};
            let client = app.world_mut().spawn(ExampleClient {
                client_id: cli.client_id().expect("You need to specify a client_id via `-c ID`"),
                client_port: CLIENT_PORT,
                server_addr: SERVER_ADDR,
                conditioner: None,
                transport: ClientTransports::Udp,
                shared: SHARED_SETTINGS,
            }).id();
            app.world_mut().trigger_targets(Connect, client)
        }
    }

    #[cfg(feature = "server")]
    {
        use lightyear_examples_common_new::server::{ExampleServer, ServerTransports};
        use lightyear::connection::server::Start;

        app.add_plugins(ExampleServerPlugin);
        if matches!(cli.mode, Some(Mode::Server)) {
            let server = app.world_mut().spawn(ExampleServer {
                conditioner: None,
                transport: ServerTransports::Udp {
                    local_port: SERVER_PORT
                },
                shared: SHARED_SETTINGS
            }).id();
            app.world_mut().trigger_targets(Start, server);
        }
    }

    #[cfg(feature = "gui")]
    app.add_plugins(renderer::ExampleRendererPlugin);

    app.run();
}
