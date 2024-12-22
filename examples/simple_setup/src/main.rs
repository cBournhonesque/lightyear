//! This minimal example showcases how to setup the lightyear plugins.
//!
//! Run with
//! - `cargo run -- server`
//! - `cargo run -- client`
#![allow(unused_imports)]
#![allow(unused_variables)]
#![allow(dead_code)]

#[cfg(feature = "client")]
mod client;
#[cfg(feature = "server")]
mod server;
mod shared;

use bevy::prelude::*;

fn main() {
    let mut app = App::new();
    #[cfg(feature = "client")]
    app.add_plugins(client::ExampleClientPlugin);
    #[cfg(feature = "server")]
    app.add_plugins(server::ExampleServerPlugin);

    app.run();
}
