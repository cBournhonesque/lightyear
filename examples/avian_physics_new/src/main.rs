#![allow(unused_imports)]
#![allow(unused_variables)]
#![allow(dead_code)]
use crate::shared::SharedPlugin;
use bevy::prelude::*;
use lightyear_examples_common::app::{Apps, Cli};

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
    // build the bevy app (this adds common plugin such as the DefaultPlugins)
    // and returns the `ClientConfig` and `ServerConfig` so that we can modify them if needed
    let mut apps = Apps::new(settings.common, cli, env!("CARGO_PKG_NAME").to_string());
    // for this example, we will use input delay and a correction function
    apps.update_lightyear_client_config(|config| {
        config
            .prediction
            .set_fixed_input_delay_ticks(settings.input_delay_ticks);
        config.prediction.correction_ticks_factor = settings.correction_ticks_factor;
    });

    apps.add_lightyear_plugins();
    apps.add_user_shared_plugin(SharedPlugin {
        predict_all: settings.predict_all,
    });
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
