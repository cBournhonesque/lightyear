use log::debug;
use std::net::SocketAddr;
use std::str::FromStr;

use lightyear_client::{Authentication, ClientConfig};
use lightyear_server::{NetcodeConfig, Server, ServerConfig};
use lightyear_shared::connection::events::IterMessageEvent;
use lightyear_shared::netcode::generate_key;
use lightyear_shared::{
    ChannelKind, IoConfig, LinkConditionerConfig, Protocol, SharedConfig, TransportConfig, World,
};
use lightyear_tests::protocol::{protocol, Channel1, Channel2, Message1};
use rand::Rng;

#[test]
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
    let io_config = IoConfig::from_transport(TransportConfig::UdpSocket(addr)).with_conditioner(
        LinkConditionerConfig {
            incoming_latency: 20,
            incoming_jitter: 10,
            incoming_loss: 0.90,
            // incoming_latency: 20,
            // incoming_jitter: 10,
            // incoming_loss: 0.1,
        },
    );
    let config = ServerConfig {
        netcode: netcode_config,
        io: io_config.clone(),
    };
    let mut server = Server::new(config, protocol());
    debug!("Created server with local address: {}", server.local_addr());

    // create client
    let client_id = 111;
    let auth = Authentication::Manual {
        server_addr: server.local_addr(),
        protocol_id,
        private_key,
        client_id,
    };
    // let addr = SocketAddr::from_str("127.0.0.1:0")?;

    let config = ClientConfig {
        shared: SharedConfig::default(),
        netcode: Default::default(),
        io: io_config,
    };
    let mut client = lightyear_client::Client::new(config, auth, protocol());
    debug!("Created client with local address: {}", client.local_addr());

    // Start the connection
    client.connect();

    let start = std::time::Instant::now();
    let tick_rate_secs = std::time::Duration::from_secs_f64(1.0 / 30.0);

    // Run the server and client in parallel
    let server_thread = std::thread::spawn(move || -> anyhow::Result<()> {
        debug!("Starting server thread");
        let mut world = World::default();
        let mut rng = rand::thread_rng();
        loop {
            server.update(start.elapsed().as_secs_f64())?;
            server.recv_packets()?;
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
                server.broadcast_send::<Channel2, Message1>(message)?;
            }
            std::thread::sleep(tick_rate_secs);
        }
    });
    let client_thread = std::thread::spawn(move || -> anyhow::Result<()> {
        let mut world = World::default();
        debug!("Starting client thread");
        loop {
            client.update(start.elapsed().as_secs_f64())?;
            client.recv_packets()?;
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
