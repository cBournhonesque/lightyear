//! # Common harness for Lightyear examples
//!
//! This module contains utilities to build a Bevy app with Lightyear plugins.
//!
//! It supports 4 different modes that can be selected using a CLI:
//! - `Server`: a single bevy [`App`] and a [`ServerConfig`] to run a dedicated server
//!    Run with `cargo run -- server`
//! - `Client`: a single bevy [`App`] and a [`ClientConfig`] to run a client
//!    Run with `cargo run -- client -c 1`
//! - `HostServer`: a single bevy [`App`] that contains both the [`ClientPlugins`] and [`ServerPlugins`].
//!    This is when you want a server to act as client as well (one of the clients is 'hosting' the game)
//!    Run with `cargo run -- host-server -c 1`
//! - `ClientAndServer`: two bevy [`App`]s that run in separate threads. One is a client, the other is a server.
//!    They will communicate via channels. This is useful for quickly testing, since you can run both
//!    the client and server with one command.
//!    Run with `cargo run -- client-and-server -c 1`
//!
//! There is a lot of setup code, but it's mostly to have the examples work in all possible configurations of transport.
//! The server supports running multiple transports at the same time (WebTransport, WebSockets, UDP, channels, Steam).
//! The client can connect to the server using any of these transports.
//!
//! ```rust,ignore
//! use bevy::prelude::{App, Plugin};
//! use lightyear_examples_common::app::{Cli, Apps};
//! use lightyear_examples_common::settings::Settings;
//!
//! struct ExampleClientPlugin;
//! struct ExampleServerPlugin;
//! #[derive(Clone)]
//! struct SharedPlugin;
//!
//! # impl Plugin for ExampleClientPlugin {
//! #    fn build(&self, app: &mut App) {}
//! # }
//! # impl Plugin for ExampleServerPlugin {
//! #    fn build(&self, app: &mut App) {}
//! # }
//! # impl Plugin for SharedPlugin {
//! #    fn build(&self, app: &mut App) {}
//! # }
//!
//!
//! fn main() {
//!     let cli = Cli::default();
//!     let settings_str = include_str!("../assets/settings.ron");
//!     let settings = common::settings::read_settings::<Settings>(settings_str);
//!     // build the bevy app (this adds common plugin such as the DefaultPlugins)
//!     Apps::new(settings, cli)
//!       // add the `ClientPlugins` and `ServerPlugins` plugin groups
//!       .add_lightyear_plugins()
//!       // update the lightyear `ClientConfig` if necessary
//!       .update_lightyear_client_config(|config| {})
//!       // add our plugins
//!       .add_user_plugins(ExampleClientPlugin, ExampleServerPlugin, SharedPlugin)
//!       // run the app
//!       .run();
//! }
//! ```
//!
//! ## Reading settings
//!
//! The settings are read from the [`settings`] module, which contains the [`Settings`](settings::Settings) struct.
//!
//! The user is expected to provide a [`Settings`](settings::Settings) struct, which can be done by manually
//! creating the struct, or by reading the settings from a file and deserializing into the struct.
//! The lightyear examples use the latter and read the settings from a RON file.
//!
//! [`App`]: app::App
//! [`ClientConfig`]: lightyear::client::config::ClientConfig
//! [`ServerConfig`]: lightyear::prelude::server::ServerConfig

// Check feature combo sanity, to make it easier to cfg out code by just checking one feature.
// For example, don't allow "steam" to be set on wasm builds, because it can't work in wasm anyway.

#[cfg(all(feature = "steam", target_family = "wasm"))]
compile_error!("steam feature is not supported in wasm");

#[cfg(all(feature = "server", target_family = "wasm"))]
compile_error!("server feature is not supported in wasm");

// Assumption: client implies gui, although technically not necessarily true,
// if you want to build a headless client to run a bot, it's possible, but unhandled here.
#[cfg(all(feature = "client", not(feature = "gui")))]
compile_error!("client feature requires gui feature");

#[cfg(all(not(feature = "client"), not(feature = "server")))]
compile_error!("One of server, client features must be enabled");

pub mod app;
#[cfg(any(feature = "bevygap_server", feature = "bevygap_client"))]
pub mod bevygap_shared;
#[cfg(feature = "gui")]
pub mod renderer;
pub mod settings;
pub mod shared;
