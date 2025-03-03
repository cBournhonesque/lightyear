#![allow(unused_imports)]
#![allow(unused_variables)]
#![allow(dead_code)]

use bevy::prelude::*;
use lightyear_examples_common::app::{Apps, Cli};

use crate::settings::get_settings;

#[cfg(feature = "client")]
mod client;
#[cfg(feature = "gui")]
mod entity_label;
#[cfg(feature = "gui")]
mod renderer;

mod protocol;
#[cfg(feature = "server")]
mod server;
mod settings;
mod shared;

fn main() {
    let cli = Cli::default();
    #[allow(unused_mut)]
    let mut settings = get_settings();
    #[cfg(target_family = "wasm")]
    lightyear_examples_common::settings::modify_digest_on_wasm(&mut settings.common.client);
    // build the bevy app (this adds common plugin such as the DefaultPlugins)
    // and returns the `ClientConfig` and `ServerConfig` so that we can modify them if needed
    let mut apps = Apps::new(settings.common, cli, env!("CARGO_PKG_NAME").to_string());
    // use input delay and a correction function to smooth over rollback errors
    apps.update_lightyear_client_config(|config| {
        config
            .prediction
            .set_fixed_input_delay_ticks(settings.input_delay_ticks);
        config.prediction.correction_ticks_factor = settings.correction_ticks_factor;
    });
    // add `ClientPlugins` and `ServerPlugins` plugin groups
    apps.add_lightyear_plugins();
    // add our plugins
    apps.add_user_shared_plugin(shared::SharedPlugin {
        show_confirmed: settings.show_confirmed,
    });
    #[cfg(feature = "client")]
    apps.add_user_client_plugin(client::ExampleClientPlugin);
    #[cfg(feature = "server")]
    apps.add_user_server_plugin(server::ExampleServerPlugin {
        predict_all: settings.predict_all,
    });
    #[cfg(feature = "gui")]
    apps.add_user_renderer_plugin(renderer::SpaceshipsRendererPlugin);
    // run the app
    apps.run();
}
