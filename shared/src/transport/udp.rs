use std::net::UdpSocket;


/// UDP Socket
pub struct Socket {
    /// The underlying UDP Socket. This is wrapped in an Arc<Mutex<>> so that it
    /// can be shared between threads
    socket: Arc<Mutex<UdpSocket>>
}