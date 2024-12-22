#![allow(unused_imports)]
#![allow(unused_variables)]
#![allow(dead_code)]
use crate::shared::SharedPlugin;
use bevy::prelude::*;
use lightyear::prelude::client::PredictionConfig;
use lightyear_examples_common::app::{Apps, Cli};
use lightyear_examples_common::settings::{read_settings, Settings};
use serde::{Deserialize, Serialize};

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
    let settings_str = include_str!("../assets/settings.ron");
    let settings = read_settings::<MySettings>(settings_str);
    // build the bevy app (this adds common plugin such as the DefaultPlugins)
    // and returns the `ClientConfig` and `ServerConfig` so that we can modify them if needed
    let mut apps = Apps::new(settings.common, cli, env!("CARGO_PKG_NAME").to_string());
    // for this example, we will use input delay and a correction function
    apps.update_lightyear_client_config(|config| {
        config.prediction.minimum_input_delay_ticks = settings.input_delay_ticks;
        config.prediction.correction_ticks_factor = settings.correction_ticks_factor;
    });

    apps.add_lightyear_plugins();
    apps.add_user_shared_plugin(SharedPlugin);
    #[cfg(feature = "client")]
    apps.add_user_client_plugin(client::ExampleClientPlugin);
    #[cfg(feature = "server")]
    apps.add_user_server_plugin(server::ExampleServerPlugin {
        predict_all: settings.predict_all,
    });
    #[cfg(feature = "gui")]
    apps.add_user_renderer_plugin(crate::renderer::ExampleRendererPlugin {
        show_confirmed: settings.show_confirmed,
    });

    // run the app
    apps.run();
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

    /// If true, we will also show the Confirmed entities (on top of the Predicted entities)
    pub(crate) show_confirmed: bool,
}
