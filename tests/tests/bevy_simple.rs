use bevy::prelude::{App, ResMut, Startup};
use std::net::SocketAddr;
use std::str::FromStr;
use tracing::{debug, info};

use lightyear_client::{Authentication, Client};
use lightyear_shared::netcode::generate_key;
use lightyear_tests::protocol::MyProtocol;

// fn client_init(mut client: ResMut<Client<MyProtocol>>) {
fn client_init() {
    panic!();
    println!("clientinit");
    dbg!("hi");
    info!("Connecting to server");
    // client.connect();
}

#[test]
fn test_simple_bevy_server_client() -> anyhow::Result<()> {
    tracing_subscriber::FmtSubscriber::builder()
        .with_max_level(tracing::Level::TRACE)
        .init();

    // Shared config
    let server_addr = SocketAddr::from_str("127.0.0.1:5000").unwrap();
    let protocol_id = 0;
    let private_key = generate_key();
    let client_id = 111;

    // Run the server and client in parallel
    // let server_thread = std::thread::spawn(move || -> anyhow::Result<()> {
    //     debug!("Starting server thread");
    //     let mut server_app = App::new();
    //     lightyear_tests::server::bevy_setup(&mut server_app, server_addr);
    //     server_app.run();
    //     Ok(())
    // });
    let client_thread = std::thread::spawn(move || -> anyhow::Result<()> {
        debug!("Starting client thread");
        let mut client_app = App::new();
        let auth = Authentication::Manual {
            server_addr,
            protocol_id,
            private_key,
            client_id,
        };
        lightyear_tests::client::bevy_setup(&mut client_app, auth);
        // client_app.add_systems(Startup, client_init);
        client_app.run();
        Ok(())
    });
    // server_thread.join().expect("server thread has panicked")?;
    client_thread.join().expect("client thread has panicked")?;
    Ok(())
}
