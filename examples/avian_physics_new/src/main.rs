#![allow(unused_imports)]
#![allow(unused_variables)]
#![allow(dead_code)]
use bevy::prelude::*;
use core::time::Duration;
use lightyear_examples_common_new::cli::{Cli, Mode};
use lightyear_examples_common_new::shared::{CLIENT_PORT, FIXED_TIMESTEP_HZ, SERVER_ADDR, SERVER_PORT, SHARED_SETTINGS};

#[cfg(feature = "client")]
use crate::client::ExampleClientPlugin;
use crate::protocol::ProtocolPlugin;
#[cfg(feature = "server")]
use crate::server::ExampleServerPlugin;

#[cfg(feature = "client")]
mod client;
mod protocol;

#[cfg(feature = "gui")]
mod renderer;
#[cfg(feature = "server")]
mod server;
// mod settings; // Settings are now handled by common_new
mod shared;

fn main() {
    let cli = Cli::default();

    let mut app = cli.build_app(
        Duration::from_secs_f64(1.0 / FIXED_TIMESTEP_HZ),
        true // Use physics loop
    );

    app.add_plugins(ProtocolPlugin);

    // NOTE: The predict_all and show_confirmed flags previously passed to plugins are not included here.
    // This might require adjustments in the client/server/renderer plugins if they need this data.

    #[cfg(feature = "client")]
    {
        app.add_plugins(ExampleClientPlugin); // Assuming ExampleClientPlugin doesn't need args now
        if matches!(cli.mode, Some(Mode::Client { .. })) {
            use lightyear::prelude::Connect;
            use lightyear_examples_common_new::client::{ClientTransports, ExampleClient};
            let client = app.world_mut().spawn(ExampleClient {
                client_id: cli.client_id().expect("You need to specify a client_id via `-c ID`"),
                client_port: CLIENT_PORT,
                server_addr: SERVER_ADDR,
                conditioner: None,
                transport: ClientTransports::Udp, // Avian example likely uses UDP
                shared: SHARED_SETTINGS,
            }).id();
            app.world_mut().trigger_targets(Connect, client)
        }
    }

    #[cfg(feature = "server")]
    {
        use lightyear_examples_common_new::server::{ExampleServer, ServerTransports};
        use lightyear::connection::server::Start;

        app.add_plugins(ExampleServerPlugin); // Assuming ExampleServerPlugin doesn't need args now
        if matches!(cli.mode, Some(Mode::Server)) {
            let server = app.world_mut().spawn(ExampleServer {
                conditioner: None,
                transport: ServerTransports::Udp { // Avian example likely uses UDP
                    local_port: SERVER_PORT
                },
                shared: SHARED_SETTINGS
            }).id();
            app.world_mut().trigger_targets(Start, server);
        }
    }

    #[cfg(feature = "gui")]
    app.add_plugins(crate::renderer::ExampleRendererPlugin); // Assuming ExampleRendererPlugin doesn't need args now

    // run the app
    app.run();
}
