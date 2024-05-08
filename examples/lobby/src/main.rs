//! This example showcases how to use Lightyear with Bevy, to easily get replication along with prediction/interpolation working.
//!
//! There is a lot of setup code, but it's mostly to have the examples work in all possible configurations of transport.
//! (all transports are supported, as well as running the example in listen-server or host-server mode)
//!
//!
//! Run with
//! - `cargo run -- server`
//! - `cargo run -- client -c 1`
#![allow(unused_imports)]
#![allow(unused_variables)]
#![allow(dead_code)]
use crate::client::ExampleClientPlugin;
use crate::server::ExampleServerPlugin;
use crate::shared::SharedPlugin;
use bevy::prelude::*;
use common::app::{Apps, Cli};
use common::settings::{ServerTransports, Settings};
use lightyear::prelude::{Deserialize, Serialize};

mod client;
mod protocol;
mod server;
mod shared;

pub const HOST_SERVER_PORT: u16 = 5050;

fn main() {
    let mut cli = common::app::cli();
    let settings_str = include_str!("../assets/settings.ron");
    let mut settings = common::settings::settings::<Settings>(settings_str);

    // in this example, every client will actually launch in host-server mode
    // the reason is that we want every client to be able to be the 'host' of a lobby
    // so every client needs to have the ServerPlugins included in the app
    match cli {
        Cli::Client { client_id } => {
            cli = Cli::HostServer { client_id };
            // when the client acts as host, we will use port UDP:5050 for the transport
            settings.server.transport = vec![ServerTransports::Udp {
                local_port: HOST_SERVER_PORT,
            }];
        }
        Cli::Server => {}
        _ => {
            panic!("This example only supports the modes Client and Server");
        }
    }

    // build the bevy app (this adds common plugins such as the DefaultPlugins)
    // and returns the `ClientConfig` and `ServerConfig` so that we can modify them
    let mut app = common::app::build_app(settings.clone(), cli);
    // we do not modify the configurations of the plugins, so we can just build
    // the `ClientPlugins` and `ServerPlugins` plugin groups
    app.add_lightyear_plugin_groups();
    // add our plugins
    app.add_plugins(
        ExampleClientPlugin { settings },
        ExampleServerPlugin,
        SharedPlugin,
    );

    // run the app
    app.run();
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct MySettings {
    pub common: Settings,

    /// If true, we will predict the client's entities, but also the ball and other clients' entities!
    /// This is what is done by RocketLeague (see [video](https://www.youtube.com/watch?v=ueEmiDM94IE))
    ///
    /// If false, we will predict the client's entities but simple interpolate everything else.
    pub(crate) predict_all: bool,

    /// By how many ticks an input press will be delayed?
    /// This can be useful as a tradeoff between input delay and prediction accuracy.
    /// If the input delay is greater than the RTT, then there won't ever be any mispredictions/rollbacks.
    /// See [this article](https://www.snapnet.dev/docs/core-concepts/input-delay-vs-rollback/) for more information.
    pub(crate) input_delay_ticks: u16,

    /// If visual correction is enabled, we don't instantly snapback to the corrected position
    /// when we need to rollback. Instead we interpolated between the current position and the
    /// corrected position.
    /// This controls the duration of the interpolation; the higher it is, the longer the interpolation
    /// will take
    pub(crate) correction_ticks_factor: f32,
}
