use std::net::SocketAddr;
use std::str::FromStr;

use bevy::log::LogPlugin;
use bevy::prelude::{App, Commands, PluginGroup, ResMut, Startup};
use bevy::winit::WinitPlugin;
use bevy::DefaultPlugins;
use tracing::{debug, info};
use tracing_subscriber::fmt::format::FmtSpan;

use lightyear_shared::client::{Authentication, Client};
use lightyear_shared::netcode::generate_key;
use lightyear_shared::replication::Replicate;
use lightyear_shared::ChannelKind;
use lightyear_tests::protocol::{Channel2, MyProtocol};

fn client_init(mut client: ResMut<Client<MyProtocol>>) {
    info!("Connecting to server");
    client.connect();
}

fn server_init(mut commands: Commands) {
    info!("Spawning entity on server");
    commands.spawn(Replicate {
        updates_channel: ChannelKind::of::<Channel2>(),
        ..Default::default()
    });
}

// fn server_init(world: &mut World) {
//     info!("Spawning entity on server");
//     std::thread::sleep(Duration::from_secs(1));
//     let replicate = Replicate::<Channel2>::default();
//     let entity = world.spawn(replicate.clone()).id();
//     let mut server = world.resource_mut::<Server<MyProtocol>>();
//     server.entity_spawn(entity, vec![], &replicate).unwrap();
// }

#[test]
fn test_simple_bevy_server_client() -> anyhow::Result<()> {
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
        server_app.add_plugins(
            DefaultPlugins
                .build()
                .disable::<LogPlugin>()
                .disable::<WinitPlugin>(),
        );
        lightyear_tests::server::bevy_setup(&mut server_app, server_addr, protocol_id, private_key);
        server_app.add_systems(Startup, server_init);
        server_app.run();
        debug!("finish server run");
        Ok(())
    });
    let client_thread = std::thread::spawn(move || -> anyhow::Result<()> {
        debug!("Starting client thread");
        let mut client_app = App::new();
        client_app.add_plugins(
            DefaultPlugins
                .build()
                .disable::<LogPlugin>()
                .disable::<WinitPlugin>(),
        );
        let auth = Authentication::Manual {
            server_addr,
            protocol_id,
            private_key,
            client_id,
        };
        lightyear_tests::client::bevy_setup(&mut client_app, auth);
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
