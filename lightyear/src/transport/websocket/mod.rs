//! Transport using the WebSocket protocol (based on TCP, HTTP)
cfg_if::cfg_if! {
    if #[cfg(all(feature = "websocket", target_family = "wasm"))] {
            pub mod client_wasm;
            pub use client_wasm as client;
    } else if #[cfg(all(feature = "websocket", not(target_family = "wasm")))]{
            pub mod server;
            pub mod client_native;
            pub use client_native as client;
    }
}

/*
API must have changed at some point, this test doesn't compile.

#[cfg(test)]
mod tests {
    use bevy::utils::Duration;

    use crate::transport::{PacketReceiver, PacketSender};

    use super::client::*;
    use super::server::*;

    #[cfg(not(target_family = "wasm"))]
    #[tokio::test]
    async fn test_websocket_native() {
        let server_addr = "127.0.0.1:7000".parse().unwrap();

        let client_socket = WebSocketClientSocket::new(server_addr);
        let server_socket = WebSocketServerSocket::new(server_addr);

        let (mut server_send, mut server_recv) = server_socket.listen();
        let (mut client_send, mut client_recv) = client_socket.listen();

        let msg = b"hello world";

        // client to server
        client_send.send(msg, &server_addr)?;

        // sleep a little to give time to the message to arrive in the socket
        tokio::time::sleep(Duration::from_millis(20)).await;

        if let Ok(Some((recv_msg, address))) = server_recv.recv() {
            assert_eq!(recv_msg, msg);

            // server to client
            server_send.send(msg, &address).unwrap();
        } else {
            panic!("server expected to receive a packet from client");
        };

        // sleep a little to give time to the message to arrive in the socket
        tokio::time::sleep(Duration::from_millis(20)).await;

        let Some((recv_msg, address)) = client_recv.recv() else {
            panic!("client expected to receive a packet from server");
        };
        assert_eq!(address, server_addr);
        assert_eq!(recv_msg, msg);
        dbg!(recv_msg);
    }
}
*/
