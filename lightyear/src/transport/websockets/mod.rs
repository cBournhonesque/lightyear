//! Transport using the WebSocket protocol (based on TCP, HTTP)
cfg_if::cfg_if! {
    if #[cfg(all(feature = "websockets", target_family = "wasm"))] {
            pub mod client_wasm;
            pub use client_wasm as client;
    } else if #[cfg(all(feature = "websockets", not(target_family = "wasm")))]{
            pub mod server;
            pub mod client_native;
            pub use client_native as client;
    }
}

const MTU: usize = 1472;

#[cfg(test)]
mod tests {
    use super::client::*;
    use super::server::*;
    use crate::transport::{PacketReceiver, PacketSender, Transport};
    use bevy::tasks::{IoTaskPool, TaskPoolBuilder};
    use bevy::utils::Duration;
    use tracing::info;
    use tracing_subscriber::fmt::format::FmtSpan;

    #[cfg(not(target_family = "wasm"))]
    #[tokio::test]
    async fn test_websocket_native() -> anyhow::Result<()> {
        // tracing_subscriber::FmtSubscriber::builder()
        //     .with_span_events(FmtSpan::ENTER)
        //     .with_max_level(tracing::Level::INFO)
        //     .init();
        let server_addr = "127.0.0.1:7000".parse().unwrap();
        let client_addr = "127.0.0.1:8000".parse().unwrap();

        let client_socket = WebSocketClientSocket::new(client_addr, server_addr, None);
        let server_socket = WebSocketServerSocket::new(server_addr, None);

        let (mut server_send, mut server_recv) = server_socket.listen();
        let (mut client_send, mut client_recv) = client_socket.listen();

        let msg = b"hello world";

        // client to server
        client_send.send(msg, &server_addr)?;

        // sleep a little to give time to the message to arrive in the socket
        tokio::time::sleep(Duration::from_millis(20)).await;

        let Some((recv_msg, address)) = server_recv.recv()? else {
            panic!("server expected to receive a packet from client");
        };
        assert_eq!(address, client_addr);
        assert_eq!(recv_msg, msg);

        // server to client
        server_send.send(msg, &client_addr)?;

        // sleep a little to give time to the message to arrive in the socket
        tokio::time::sleep(Duration::from_millis(20)).await;

        let Some((recv_msg, address)) = client_recv.recv()? else {
            panic!("client expected to receive a packet from server");
        };
        assert_eq!(address, server_addr);
        assert_eq!(recv_msg, msg);
        dbg!(recv_msg);
        Ok(())
    }
}