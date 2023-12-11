use std::net::SocketAddr;
use std::str::FromStr;

use bevy::prelude::{App, Commands, ResMut, Startup};
use bevy::MinimalPlugins;
use tracing::{debug, info};
use tracing_subscriber::fmt::format::FmtSpan;

use lightyear::netcode::generate_key;
use lightyear::prelude::client::{Authentication, Client};
use lightyear::prelude::*;
use lightyear_examples::protocol::MyProtocol;

fn client_init(mut client: ResMut<Client<MyProtocol>>) {
    info!("Connecting to server");
    client.connect();
}

fn server_init(mut commands: Commands) {
    info!("Spawning entity on server");
    commands.spawn(Replicate {
        ..Default::default()
    });
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::FmtSubscriber::builder()
        .with_span_events(FmtSpan::ENTER)
        .with_max_level(tracing::Level::DEBUG)
        .init();

    // Shared config
    let server_addr = SocketAddr::from_str("127.0.0.1:5000").unwrap();
    let protocol_id = 0;
    let private_key = generate_key();
    let client_id = 111;

    // Run the server and client in parallel
    let server_thread = std::thread::spawn(move || -> anyhow::Result<()> {
        debug!("Starting server thread");
        let mut server_app = App::new();
        server_app.add_plugins(MinimalPlugins);
        lightyear_examples::server::bevy_setup(
            &mut server_app,
            server_addr,
            protocol_id,
            private_key,
        );
        server_app.add_systems(Startup, server_init);
        server_app.run();
        debug!("finish server run");
        Ok(())
    });
    let client_thread = std::thread::spawn(move || -> anyhow::Result<()> {
        debug!("Starting client thread");
        let mut client_app = App::new();
        client_app.add_plugins(MinimalPlugins);
        let auth = Authentication::Manual {
            server_addr,
            protocol_id,
            private_key,
            client_id,
        };
        lightyear_examples::client::bevy_setup(&mut client_app, auth);
        client_app.add_systems(Startup, client_init);
        client_app.run();
        debug!("finish client run");
        Ok(())
    });
    client_thread.join().expect("client thread has panicked")?;
    server_thread.join().expect("server thread has panicked")?;
    debug!("OVER");
    Ok(())
}
