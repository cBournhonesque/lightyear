use bevy::prelude::{App, ResMut, Startup};
use tracing::{debug, info};

use lightyear_client::Client;
use lightyear_tests::protocol::MyProtocol;

fn client_init(mut client: ResMut<Client<MyProtocol>>) {
    println!("clientinit");
    dbg!("hi");
    info!("Connecting to server");
    client.connect();
}

#[test]
fn test_simple_bevy_server_client() -> anyhow::Result<()> {
    // tracing_subscriber::FmtSubscriber::builder()
    //     .with_max_level(tracing::Level::TRACE)
    //     .init();

    // Create the server and client
    let client_id = 111;
    let mut server_app = App::new();
    let token = lightyear_tests::server::bevy_setup(&mut server_app, client_id);
    let mut client_app = App::new();
    lightyear_tests::client::bevy_setup(&mut client_app, token);

    // Start the connection
    client_app.add_systems(Startup, client_init);

    // Run the server and client in parallel
    let server_thread = std::thread::spawn(move || -> anyhow::Result<()> {
        debug!("Starting server thread");
        server_app.run();
        Ok(())
    });
    let client_thread = std::thread::spawn(move || -> anyhow::Result<()> {
        debug!("Starting client thread");
        client_app.run();
        Ok(())
    });
    server_thread.join().expect("server thread has panicked")?;
    client_thread.join().expect("client thread has panicked")?;
    Ok(())
}
