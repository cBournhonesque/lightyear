#![allow(unused_imports)]
#![allow(unused_variables)]
#![allow(dead_code)]
#[cfg(feature = "client")]
use crate::client::ExampleClientPlugin;
#[cfg(feature = "server")]
use crate::server::ExampleServerPlugin;
use crate::shared::SharedPlugin;
use bevy::prelude::*;
use core::time::Duration;
use lightyear_examples_common::cli::{Cli, Mode};
use lightyear_examples_common::shared::{
    CLIENT_PORT, FIXED_TIMESTEP_HZ, SERVER_ADDR, SERVER_PORT, SHARED_SETTINGS,
};

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

    let mut app = cli.build_app(Duration::from_secs_f64(1.0 / FIXED_TIMESTEP_HZ), true);
    
    app.add_plugins(SharedPlugin { predict_all: true });

    #[cfg(feature = "client")]
    {
        app.add_plugins(ExampleClientPlugin);
        if matches!(cli.mode, Some(Mode::Client { .. })) {
            use lightyear::prelude::Connect;
            use lightyear_examples_common::client::{ClientTransports, ExampleClient};
            let client = app
                .world_mut()
                .spawn(ExampleClient {
                    client_id: cli
                        .client_id()
                        .expect("You need to specify a client_id via `-c ID`"),
                    client_port: CLIENT_PORT,
                    server_addr: SERVER_ADDR,
                    conditioner: None,
                    transport: ClientTransports::Udp,
                    shared: SHARED_SETTINGS,
                })
                .id();
            app.world_mut().trigger_targets(Connect, client)
        }
    }

    #[cfg(feature = "server")]
    {
        use lightyear::connection::server::Start;
        use lightyear_examples_common::server::{ExampleServer, ServerTransports};

        app.add_plugins(ExampleServerPlugin);
        if matches!(cli.mode, Some(Mode::Server)) {
            let server = app
                .world_mut()
                .spawn(ExampleServer {
                    conditioner: None,
                    transport: ServerTransports::Udp {
                        local_port: SERVER_PORT,
                    },
                    shared: SHARED_SETTINGS,
                })
                .id();
            app.world_mut().trigger_targets(Start, server);
        }
    }
    
    

    #[cfg(feature = "gui")]
    {
        app.add_plugins(renderer::ExampleRendererPlugin {
            show_confirmed: true,
        });
        // app.add_plugins(bevy_metrics_dashboard::RegistryPlugin::default())
        //     .add_plugins(bevy_metrics_dashboard::DashboardPlugin);
        // app.world_mut()
        //     .spawn(bevy_metrics_dashboard::DashboardWindow::new(
        //         "Metrics Dashboard",
        //     ));
    }

    // run the app
    app.run();
}
