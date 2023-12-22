//! The connection soak test how sending messages/packets works with a real connection, and loss/jitter
//! We put this test here because it uses some private methods.
use std::net::SocketAddr;
use std::str::FromStr;
use std::time::Duration;

use bevy::ecs::component::Tick as BevyTick;
use bevy::prelude::World;
use rand::Rng;
use tracing::debug;

use crate::connection::events::IterMessageEvent;
use crate::prelude::client::{Authentication, Client, ClientConfig, SyncConfig};
use crate::prelude::server::{NetcodeConfig, Server, ServerConfig};
use crate::prelude::*;
use crate::tests::protocol::*;

#[test]
#[ignore]
fn test_connection_soak() -> anyhow::Result<()> {
    tracing_subscriber::FmtSubscriber::builder()
        .with_max_level(tracing::Level::DEBUG)
        .init();

    // common
    let protocol_id = 0;
    let private_key = generate_key();

    // Create server
    let addr = SocketAddr::from_str("127.0.0.1:0")?;
    let netcode_config = NetcodeConfig::default()
        .with_protocol_id(protocol_id)
        .with_key(private_key);
    let io_server = Io::from_config(
        &IoConfig::from_transport(TransportConfig::UdpSocket(addr)).with_conditioner(
            LinkConditionerConfig {
                incoming_latency: Duration::from_millis(20),
                incoming_jitter: Duration::from_millis(10),
                incoming_loss: 0.10,
            },
        ),
    );
    let fixed_timestep = Duration::from_millis(10);
    // TODO: link conditioner doesn't work with virtual time
    let shared_config = SharedConfig {
        enable_replication: false,
        tick: TickConfig::new(fixed_timestep),
        ..Default::default()
    };
    let config = ServerConfig {
        shared: shared_config.clone(),
        netcode: netcode_config,
        ping: Default::default(),
    };
    let mut server = Server::new(config, io_server, protocol());
    debug!("Created server with local address: {}", server.local_addr());

    // create client
    let client_id = 111;
    let auth = Authentication::Manual {
        server_addr: server.local_addr(),
        protocol_id,
        private_key,
        client_id,
    };
    let io_client = Io::from_config(
        &IoConfig::from_transport(TransportConfig::UdpSocket(addr)).with_conditioner(
            LinkConditionerConfig {
                incoming_latency: Duration::from_millis(20),
                incoming_jitter: Duration::from_millis(10),
                incoming_loss: 0.10,
            },
        ),
    );
    let config = ClientConfig {
        shared: shared_config.clone(),
        netcode: Default::default(),
        ping: Default::default(),
        sync: SyncConfig::default(),
        ..Default::default()
    };
    let mut client = Client::new(config, io_client, auth, protocol());
    debug!("Created client with local address: {}", client.local_addr());

    // Start the connection
    client.connect();

    let start = std::time::Instant::now();
    let tick_rate_secs = Duration::from_secs_f64(1.0 / 30.0);

    // Run the server and client in parallel
    let server_thread = std::thread::spawn(move || -> anyhow::Result<()> {
        debug!("Starting server thread");
        let mut world = World::default();
        let mut rng = rand::thread_rng();
        loop {
            server.update(start.elapsed())?;
            server.recv_packets(BevyTick::new(0))?;
            server.send_packets()?;
            server.receive(&mut world);

            let num_message = rng.gen_range(0..2);
            // let num_message = 0;
            for _ in 0..num_message {
                // TODO: use geometric distribution? use multiple of FRAGMENT_SIZE?

                // TODO: there is a problem with fragments, issues only appear with fragments
                let message_length = rng.gen_range(0..1300);
                let s: String = (&mut rng)
                    .sample_iter(rand::distributions::Alphanumeric)
                    .take(message_length)
                    .map(char::from)
                    .collect();
                let message = Message1(s);
                debug!("Sending message {message:?}");
                server.send_to_target::<Channel1, Message1>(message, NetworkTarget::All)?;
            }
            std::thread::sleep(tick_rate_secs);
        }
    });
    let client_thread = std::thread::spawn(move || -> anyhow::Result<()> {
        let mut world = World::default();
        debug!("Starting client thread");
        loop {
            // can use 0 overstep if not in Bevy
            client.update(start.elapsed(), Duration::default())?;
            client.recv_packets(BevyTick::new(0))?;
            client.send_packets()?;

            if client.is_connected() {
                let mut events = client.receive(&mut world);
                if !events.is_empty() {
                    debug!("events received: {:?}", events);
                    for (message, _) in events.into_iter_messages::<Message1>() {
                        debug!("Received message {message:?}");
                    }
                }
            }
            std::thread::sleep(tick_rate_secs);
        }
    });
    client_thread.join().expect("client thread has panicked")?;
    server_thread.join().expect("server thread has panicked")?;
    Ok(())
}
