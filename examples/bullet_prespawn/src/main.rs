#![allow(unused_imports)]
#![allow(unused_variables)]
#![allow(dead_code)]
use crate::client::ExampleClientPlugin;
use crate::server::ExampleServerPlugin;
use crate::shared::SharedPlugin;
use bevy::prelude::*;
use common::app::Apps;
use common::settings::{settings, Settings};
use lightyear::prelude::client::PredictionConfig;
use serde::{Deserialize, Serialize};

mod client;
mod protocol;
mod server;
mod shared;

fn main() {
    let cli = common::app::cli();
    let settings_str = include_str!("../assets/settings.ron");
    let settings = settings::<MySettings>(settings_str);
    // build the bevy app (this adds common plugin such as the DefaultPlugins)
    // and returns the `ClientConfig` and `ServerConfig` so that we can modify them if needed
    let mut app = common::app::build_app(settings.common, cli);

    // for this example, we will use input delay and a correction function
    let prediction_config = PredictionConfig {
        input_delay_ticks: settings.input_delay_ticks,
        correction_ticks_factor: settings.correction_ticks_factor,
        ..default()
    };
    match &mut app {
        Apps::Client { config, .. } => {
            config.prediction = prediction_config;
        }
        Apps::ListenServer { client_config, .. } => {
            client_config.prediction = prediction_config;
        }
        Apps::HostServer { client_config, .. } => {
            client_config.prediction = prediction_config;
        }
        _ => {}
    }
    // add `ClientPlugins` and `ServerPlugins` plugin groups
    app.add_lightyear_plugin_groups();
    // add our plugins
    app.add_plugins(ExampleClientPlugin, ExampleServerPlugin, SharedPlugin);
    // run the app
    app.run();
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct MySettings {
    pub common: Settings,

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
