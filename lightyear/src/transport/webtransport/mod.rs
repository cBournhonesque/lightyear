//! Transport using the WebTransport protocol (based on QUIC)
cfg_if::cfg_if! {
    if #[cfg(all(feature = "webtransport", target_family = "wasm"))] {
            pub mod client_wasm;
            pub use client_wasm as client;
    } else if #[cfg(all(feature = "webtransport", not(target_family = "wasm")))]{
            pub mod server;
            pub mod client_native;
            pub use client_native as client;
    }
}

#[cfg(test)]
mod tests {
    use crate::client::io::transport::ClientTransportBuilder;
    use crate::server::io::transport::ServerTransportBuilder;
    use bevy::tasks::{IoTaskPool, TaskPoolBuilder};
    use core::time::Duration;
    use wtransport::Identity;
    use crate::transport::Transport;

    use super::client::*;
    use super::server::*;

    #[cfg(not(target_family = "wasm"))]
    #[tokio::test]
    async fn test_webtransport_native() {
        // tracing_subscriber::FmtSubscriber::builder()
        //     .with_span_events(FmtSpan::ENTER)
        //     .with_max_level(tracing::Level::INFO)
        //     .init();
        IoTaskPool::get_or_init(|| TaskPoolBuilder::default().build());

        let certificate = Identity::self_signed(["localhost"]).unwrap();
        let server_addr = "127.0.0.1:7000".parse().unwrap();
        let client_addr = "127.0.0.1:8000".parse().unwrap();

        let (server_socket, _, a, b) = WebTransportServerSocketBuilder {
            server_addr,
            certificate,
        }
        .start()
        .unwrap();
        let (mut server_send, mut server_recv) = server_socket.split();

        let (client_socket, _, c, d) = WebTransportClientSocketBuilder {
            client_addr,
            server_addr,
        }
        .connect()
        .unwrap();
        let (mut client_send, mut client_recv) = client_socket.split();

        let msg = b"hello world";

        // client to server
        client_send.send(msg, &server_addr).unwrap();

        // sleep a little to give time to the message to arrive in the socket
        tokio::time::sleep(Duration::from_millis(20)).await;

        let Ok(Some((recv_msg, address))) = server_recv.recv() else {
            panic!("server expected to receive a packet from client");
        };
        assert_eq!(address, client_addr);
        assert_eq!(recv_msg, msg);

        // server to client
        server_send.send(msg, &client_addr).unwrap();

        // sleep a little to give time to the message to arrive in the socket
        tokio::time::sleep(Duration::from_millis(20)).await;

        let Ok(Some((recv_msg, address))) = client_recv.recv() else {
            panic!("client expected to receive a packet from server");
        };
        assert_eq!(address, server_addr);
        assert_eq!(recv_msg, msg);
    }
}

#[cfg(target_family = "wasm")]
#[cfg(test)]
pub mod wasm_test {
    use wasm_bindgen_test::*;

    #[wasm_bindgen_test]
    #[tokio::test]
    async fn test_webtransport_wasm() {
        // tracing_subscriber::FmtSubscriber::builder()
        //     .with_span_events(FmtSpan::ENTER)
        //     .with_max_level(tracing::Level::INFO)
        //     .init();
        let certificate = Certificate::self_signed(["localhost"]);
        let server_addr = "127.0.0.1:7000".parse().unwrap();
        let client_addr = "127.0.0.1:8000".parse().unwrap();

        let mut client_socket = WebTransportClientSocket::new(client_addr, server_addr);
        let mut server_socket = WebTransportServerSocket::new(server_addr, certificate);

        let (mut server_send, mut server_recv) = server_socket.listen();
        let (mut client_send, mut client_recv) = client_socket.listen();

        let msg = b"hello world";

        // client to server
        client_send.send(msg, &server_addr)?;

        // sleep a little to give time to the message to arrive in the socket
        tokio::time::sleep(Duration::from_millis(20)).await;

        let Ok(Some((recv_msg, address))) = server_recv.recv() else {
            panic!("server expected to receive a packet from client");
        };
        assert_eq!(address, client_addr);
        assert_eq!(recv_msg, msg);

        // server to client
        server_send.send(msg, &client_addr).unwrap();

        // sleep a little to give time to the message to arrive in the socket
        tokio::time::sleep(Duration::from_millis(20)).await;

        let Ok(Some((recv_msg, address))) = client_recv.recv() else {
            panic!("client expected to receive a packet from server");
        };
        assert_eq!(address, server_addr);
        assert_eq!(recv_msg, msg);
    }
}
