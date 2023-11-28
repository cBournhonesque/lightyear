#![allow(unused_imports)]
#![allow(unused_variables)]
#![allow(dead_code)]

use bevy::prelude::World;
use log::debug;
use std::time::Duration;

use lightyear_shared::prelude::client::Authentication;
use lightyear_shared::prelude::*;
use lightyear_tests::protocol::{Channel2, Message1, MyMessageProtocol};

#[test]
fn test_simple_server_client() -> anyhow::Result<()> {
    tracing_subscriber::FmtSubscriber::builder()
        .with_max_level(tracing::Level::DEBUG)
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
    let tick_rate_secs = Duration::from_secs_f64(1.0 / 30.0);

    let message1 = Message1("Hello World".to_string());
    let message1_expected = MyMessageProtocol::Message1(message1.clone());
    // let channel_kind_1 = ChannelKind::of::<Channel1>();
    let channel_kind_2 = ChannelKind::of::<Channel2>();

    // Run the server and client in parallel
    let server_thread = std::thread::spawn(move || -> anyhow::Result<()> {
        debug!("Starting server thread");
        let mut world = World::default();
        loop {
            server.update(start.elapsed())?;
            server.recv_packets()?;
            server.send_packets()?;

            let events = server.receive(&mut world);

            // if events.has_messages::<Message1>() {
            //     let messages = events
            //         .events
            //         .get(&client_id)
            //         .unwrap()
            //         .messages
            //         .get(&MessageKind::of::<Message1>());
            //     assert_eq!(
            //         messages,
            //         Some(
            //             &vec![(channel_kind_2, vec![message1_expected])]
            //                 .into_iter()
            //                 .collect()
            //         )
            //     );
            //     break;
            // }

            std::thread::sleep(tick_rate_secs);
        }
        Ok(())
    });
    let client_thread = std::thread::spawn(move || -> anyhow::Result<()> {
        debug!("Starting client thread");
        let mut world = World::default();
        loop {
            client.update(start.elapsed(), Duration::default())?;
            client.recv_packets()?;
            client.send_packets()?;

            client.receive(&mut world);

            // if client.is_connected() {
            //     client.buffer_send::<Channel2, Message1>(message1)?;
            //     client.send_packets()?;
            //     break;
            // }
            std::thread::sleep(tick_rate_secs);
        }
        Ok(())
    });
    client_thread.join().expect("client thread has panicked")?;
    server_thread.join().expect("server thread has panicked")?;
    Ok(())
}
