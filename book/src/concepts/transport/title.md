# Transport

The `Transport` trait is the trait that is used to send and receive raw data on the network.

It is very general:
```rust,noplayground
pub trait PacketSender: Send + Sync {
    /// Send data on the socket to the remote address
    fn send(&mut self, payload: &[u8], address: &SocketAddr) -> Result<()>;
}
pub trait PacketReceiver: Send + Sync {
    /// Receive a packet from the socket. Returns the data read and the origin.
    ///
    /// Returns Ok(None) if no data is available
    fn recv(&mut self) -> Result<Option<(&mut [u8], SocketAddr)>>;
}
```


The trait currently has 3 implementations:
- UDP sockets
- WebTransport (using QUIC): not compatible with wasm yet.
- crossbeam-channels: used for internal testing
