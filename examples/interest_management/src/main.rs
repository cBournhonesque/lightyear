#![allow(unused_imports)]
#![allow(unused_variables)]
#![allow(dead_code)]
use crate::client::ExampleClientPlugin;
use crate::server::ExampleServerPlugin;
use crate::shared::SharedPlugin;
use bevy::prelude::*;
use lightyear_examples_common::app::Apps;
use lightyear_examples_common::settings::{read_settings, Settings};

mod client;
mod protocol;
mod server;
mod shared;

fn main() {
    let cli = lightyear_examples_common::app::cli();
    let settings_str = include_str!("../assets/settings.ron");
    let settings = read_settings::<Settings>(settings_str);
    // build the bevy app (this adds common plugin such as the DefaultPlugins)
    let mut apps = Apps::new(settings, cli, env!("CARGO_PKG_NAME").to_string());
    // add `ClientPlugins` and `ServerPlugins` plugin groups
    apps.add_lightyear_plugins()
        // add our plugins
        .add_user_plugins(ExampleClientPlugin, ExampleServerPlugin, SharedPlugin);
    // run the app
    apps.run();
}
