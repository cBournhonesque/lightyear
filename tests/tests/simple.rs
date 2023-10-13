use log::debug;

use lightyear_shared::netcode::ClientIndex;
use lightyear_shared::{ChannelKind, MessageContainer};
use lightyear_tests::protocol::{Channel2, Message1, MyMessageProtocol};

#[test]
fn test_simple_server_client() -> anyhow::Result<()> {
    tracing_subscriber::FmtSubscriber::builder()
        .with_max_level(tracing::Level::TRACE)
        .init();

    // Create the server and client
    let client_id = 111;
    let mut server = lightyear_tests::server::setup()?;
    debug!("Created server with local address: {}", server.local_addr());
    let mut client = lightyear_tests::client::setup(server.token(client_id))?;
    debug!("Created client with local address: {}", client.local_addr());

    // Start the connection
    client.connect();

    let start = std::time::Instant::now();
    let tick_rate_secs = std::time::Duration::from_secs_f64(1.0 / 30.0);

    let message1 = MessageContainer::new(MyMessageProtocol::Message1(Message1(
        "Hello World".to_string(),
    )));
    let message1_expected = message1.clone();
    // let channel_kind_1 = ChannelKind::of::<Channel1>();
    let channel_kind_2 = ChannelKind::of::<Channel2>();

    // Run the server and client in parallel
    let server_thread = std::thread::spawn(move || -> anyhow::Result<()> {
        debug!("Starting server thread");
        loop {
            server.update(start.elapsed().as_secs_f64())?;
            server.recv_packets()?;
            server.send_packets()?;

            let client_index = ClientIndex(0);
            let messages = server.read_messages(client_index);
            if !messages.is_empty() {
                assert_eq!(
                    messages.get(&channel_kind_2),
                    Some(&vec![message1_expected])
                );
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
                client.buffer_send(message1, channel_kind_2)?;
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
