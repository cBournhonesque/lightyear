//! This minimal example showcases how to setup the lightyear plugins.
//!
//! Run with
//! - `cargo run -- server`
//! - `cargo run -- client`
#![allow(unused_imports)]
#![allow(unused_variables)]
#![allow(dead_code)]

mod automation;
#[cfg(feature = "client")]
mod client;
#[cfg(feature = "server")]
mod server;
mod shared;

#[cfg(feature = "client")]
use crate::automation::ClientStartupConfig;
#[cfg(feature = "server")]
use crate::automation::ServerStartupConfig;
use crate::shared::{SharedPlugin, FIXED_TIMESTEP_HZ};
use bevy::prelude::*;
use clap::{Parser, Subcommand, ValueEnum};
use core::time::Duration;
#[cfg(feature = "server")]
use lightyear::connection::server::Start;
#[cfg(feature = "client")]
use lightyear::prelude::client::ClientPlugins;
#[cfg(feature = "server")]
use lightyear::prelude::server::ServerPlugins;
#[cfg(all(feature = "server", not(feature = "webtransport"), feature = "udp"))]
use lightyear::prelude::server::ServerUdpIo;
#[cfg(all(
    feature = "server",
    feature = "webtransport",
    not(target_family = "wasm")
))]
use lightyear::prelude::server::WebTransportServerIo;
#[cfg(feature = "server")]
use lightyear::prelude::server::{NetcodeConfig as ServerNetcodeConfig, NetcodeServer};
#[cfg(feature = "server")]
use lightyear::prelude::*;

/// CLI options to create an [`App`]
#[derive(Parser, Debug)]
#[command(version, about)]
pub struct Cli {
    #[command(subcommand)]
    pub mode: Mode,
}

#[derive(Subcommand, Debug)]
pub enum Mode {
    #[cfg(feature = "client")]
    Client,
    #[cfg(feature = "server")]
    Server,
    #[cfg(all(feature = "client", feature = "server"))]
    HostClient {
        #[arg(short, long, default_value_t = 0)]
        client_id: u64,
    },
}

fn main() {
    let cli = cli();
    let mut app = automation::build_base_app();

    match cli.mode {
        #[cfg(feature = "client")]
        Mode::Client => {
            // add lightyear plugins
            app.add_plugins(ClientPlugins {
                tick_duration: Duration::from_secs_f64(1.0 / FIXED_TIMESTEP_HZ),
            });
            // ProtocolPlugin must be added after the Client/Server plugins.
            app.add_plugins(SharedPlugin);
            app.insert_resource(ClientStartupConfig {
                client_id: 0,
                host_server: None,
            });
            // add client-specific plugins
            app.add_plugins(client::ExampleClientPlugin);
        }
        #[cfg(feature = "server")]
        Mode::Server => {
            app.add_plugins(ServerPlugins {
                tick_duration: Duration::from_secs_f64(1.0 / FIXED_TIMESTEP_HZ),
            });
            // ProtocolPlugin must be added after the Client/Server plugins.
            app.add_plugins(SharedPlugin);
            app.insert_resource(ServerStartupConfig::default());
            app.add_plugins(server::ExampleServerPlugin);
        }
        #[cfg(all(feature = "client", feature = "server"))]
        Mode::HostClient { client_id } => {
            app.add_plugins(ClientPlugins {
                tick_duration: Duration::from_secs_f64(1.0 / FIXED_TIMESTEP_HZ),
            });
            app.add_plugins(ServerPlugins {
                tick_duration: Duration::from_secs_f64(1.0 / FIXED_TIMESTEP_HZ),
            });
            app.add_plugins(SharedPlugin);
            app.add_plugins(server::ExampleServerPlugin);
            app.insert_resource(ServerStartupConfig { auto_spawn: false });
            let server = app
                .world_mut()
                .spawn((
                    NetcodeServer::new(ServerNetcodeConfig::default()),
                    LocalAddr(shared::SERVER_ADDR),
                    #[cfg(all(feature = "webtransport", not(target_family = "wasm")))]
                    WebTransportServerIo {
                        certificate: shared::webtransport_self_signed_certificate(),
                    },
                    #[cfg(all(not(feature = "webtransport"), feature = "udp"))]
                    ServerUdpIo::default(),
                ))
                .id();
            app.world_mut().trigger(Start { entity: server });
            app.insert_resource(ClientStartupConfig {
                client_id,
                host_server: Some(server),
            });
            app.add_plugins(client::ExampleClientPlugin);
        }
    }
    app.run();
}

fn cli() -> Cli {
    cfg_if::cfg_if! {
        if #[cfg(target_family = "wasm")] {
            Cli { mode: Mode::Client }
        } else {
            Cli::parse()
        }
    }
}
