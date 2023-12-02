use crate::transport::PacketSender;
use bevy::tasks::TaskPool;
use std::net::SocketAddr;
use tokio::sync::mpsc;
use tracing::error;
use wtransport;
use wtransport::datagram::Datagram;
use wtransport::ClientConfig;

/// UDP Socket
// #[derive(Clone)]
pub struct WebTransportConnection {
    /// The underlying UDP Socket. This is wrapped in an Arc<Mutex<>> so that it
    /// can be shared between threads
    // buffer: [u8; MTU],
    // session: Session,
    // config: ClientConfig,
    from_server_receiver: mpsc::UnboundedReceiver<Datagram>,
    to_server_sender: mpsc::UnboundedSender<Vec<u8>>,
}

impl WebTransportConnection {
    pub fn connect(addr: SocketAddr) -> Self {
        let config = ClientConfig::builder()
            .with_bind_default()
            .with_no_cert_validation()
            .build();
        let server_addr = "127.0.0.1:5000";
        let (to_server_sender, mut to_server_receiver) = mpsc::unbounded_channel::<Vec<u8>>();
        let (from_server_sender, from_server_receiver) = mpsc::unbounded_channel();

        let executor = TaskPool::get_thread_executor().spawn(async move {
            let connection = wtransport::Endpoint::client(config)
                .unwrap()
                .connect(server_addr)
                .await
                .unwrap();

            loop {
                tokio::select! {
                    // receive messages from server
                    x = connection.receive_datagram() => {
                        match x {
                            Ok(data) => {
                                from_server_sender.send(data).unwrap();
                            }
                            Err(e) => {
                                error!("receive_datagram error: {:?}", e);
                            }
                        }
                    }

                    // send messages to server
                    Some(msg) = to_server_receiver.recv() => {
                        connection.send_datagram(msg.as_slice()).unwrap_or_else(|e| {
                            error!("send_datagram error: {:?}", e);
                        });
                    }
                }
            }
        });
        Self {
            // config,
            from_server_receiver,
            to_server_sender,
        }
    }
}

impl PacketSender for WebTransportConnection {
    fn send(&mut self, payload: &[u8], address: &SocketAddr) -> std::io::Result<()> {
        todo!()
    }
}
