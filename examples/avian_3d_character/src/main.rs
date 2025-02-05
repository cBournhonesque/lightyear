#![allow(unused_imports)]
#![allow(unused_variables)]
#![allow(dead_code)]
use crate::shared::SharedPlugin;
use bevy::prelude::*;
use lightyear::prelude::client::PredictionConfig;
use lightyear_examples_common::app::{Apps, Cli, Mode};
use lightyear_examples_common::settings::Settings;
use serde::{Deserialize, Serialize};

#[cfg(feature = "client")]
mod client;
mod protocol;
#[cfg(feature = "gui")]
mod renderer;
#[cfg(feature = "server")]
mod server;
mod settings;
mod shared;

fn main() {
    let cli = Cli::default();
    let settings = settings::get_settings();
    let mut apps = Apps::new(settings.common, cli, env!("CARGO_PKG_NAME").to_string());

    apps.update_lightyear_client_config(|config| {
        config
            .prediction
            .set_fixed_input_delay_ticks(settings.input_delay_ticks);
        config.prediction.correction_ticks_factor = settings.correction_ticks_factor;
    });

    apps.add_lightyear_plugins();
    apps.add_user_shared_plugin(SharedPlugin);
    #[cfg(feature = "server")]
    apps.add_user_server_plugin(server::ExampleServerPlugin);
    #[cfg(feature = "client")]
    apps.add_user_client_plugin(client::ExampleClientPlugin);
    #[cfg(feature = "gui")]
    apps.add_user_renderer_plugin(renderer::ExampleRendererPlugin);

    apps.run();
}
