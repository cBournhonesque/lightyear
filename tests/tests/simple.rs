use log::debug;

use lightyear_client::Authentication;
use lightyear_shared::netcode::generate_key;
use lightyear_shared::{ChannelKind, World};
use lightyear_tests::protocol::{Channel2, Message1, MyMessageProtocol};

#[test]
fn test_simple_server_client() -> anyhow::Result<()> {
    tracing_subscriber::FmtSubscriber::builder()
        .with_max_level(tracing::Level::TRACE)
        .init();

    // Create the server and client
    let protocol_id = 0;
    let private_key = generate_key();
    let mut server = lightyear_tests::server::setup(protocol_id, private_key)?;
    debug!("Created server with local address: {}", server.local_addr());
    let client_id = 111;
    let auth = Authentication::Manual {
        server_addr: server.local_addr(),
        protocol_id,
        private_key,
        client_id,
    };
    let mut client = lightyear_tests::client::setup(auth)?;
    debug!("Created client with local address: {}", client.local_addr());

    // Start the connection
    client.connect();

    let start = std::time::Instant::now();
    let tick_rate_secs = std::time::Duration::from_secs_f64(1.0 / 30.0);

    let message1 = MyMessageProtocol::Message1(Message1("Hello World".to_string()));
    let message1_expected = message1.clone();
    // let channel_kind_1 = ChannelKind::of::<Channel1>();
    let channel_kind_2 = ChannelKind::of::<Channel2>();

    // Run the server and client in parallel
    let server_thread = std::thread::spawn(move || -> anyhow::Result<()> {
        debug!("Starting server thread");
        let mut World = World::default();
        loop {
            server.update(start.elapsed().as_secs_f64())?;
            server.recv_packets()?;
            server.send_packets()?;

            let events = server.receive(&mut World);

            if !events.is_empty() {
                let messages = events
                    .events
                    .get(&client_id)
                    .unwrap()
                    .messages
                    .get(&channel_kind_2);
                assert_eq!(messages, Some(&vec![message1_expected]));
                break;
            }

            std::thread::sleep(tick_rate_secs);
        }
        Ok(())
    });
    let client_thread = std::thread::spawn(move || -> anyhow::Result<()> {
        debug!("Starting client thread");
        loop {
            client.update(start.elapsed().as_secs_f64())?;
            client.recv_packets()?;
            client.send_packets()?;

            if client.is_connected() {
                client.buffer_send::<Channel2>(message1)?;
                client.send_packets()?;
                break;
            }
            std::thread::sleep(tick_rate_secs);
        }
        Ok(())
    });
    server_thread.join().expect("server thread has panicked")?;
    client_thread.join().expect("client thread has panicked")?;
    Ok(())
}
