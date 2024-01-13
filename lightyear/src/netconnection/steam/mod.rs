mod client;
mod server;

pub use client::Client;

/// The maximum size of a packet in bytes.
pub const MAX_PACKET_SIZE: usize = 1200;
