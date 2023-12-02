pub(crate) mod client;
pub(crate) mod server;

pub mod cert;

// Maximum transmission units; maximum size in bytes of a UDP packet
// See: https://gafferongames.com/post/packet_fragmentation_and_reassembly/
const MTU: usize = 1472;

#[cfg(test)]
mod tests {
    use super::client::*;
    use super::server::*;
    use crate::transport::webtransport::cert::{dump_certificate, generate_local_certificate};
    use crate::transport::{PacketReceiver, PacketSender, Transport};
    use bevy::tasks::{IoTaskPool, TaskPoolBuilder};
    use std::time::Duration;
    use tracing::info;
    use tracing_subscriber::fmt::format::FmtSpan;
    use wtransport::tls::Certificate;

    #[tokio::test]
    async fn test_webtransport_socket() -> anyhow::Result<()> {
        // tracing_subscriber::FmtSubscriber::builder()
        //     .with_span_events(FmtSpan::ENTER)
        //     .with_max_level(tracing::Level::INFO)
        //     .init();
        let certificate = generate_local_certificate();
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
        Ok(())
    }
}
