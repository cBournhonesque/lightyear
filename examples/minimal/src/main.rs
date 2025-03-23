use lightyear::io::transport::Transport;
use lightyear::connection::id::ClientId;
use lightyear::connection::client::{ClientConnection, NetClient};
use lightyear::connection::server::{ServerConnection, NetServer};
use lightyear::packet::channel::builder::{ChannelMode, ChannelDirection, ChannelBuilder};
use std::error::Error;

fn main() -> Result<(), Box<dyn Error>> {
    println!("Lightyear Minimal Example");
    
    // Create a channel
    let channel = ChannelBuilder::new(ChannelMode::ReliableOrdered, ChannelDirection::Bidirectional)
        .build::<MyChannel>();
    
    println!("Channel created with mode: {:?}", channel.settings().mode);
    
    // Create server and client connections
    let mut server_conn = ServerConnection::new();
    let mut client_conn = ClientConnection::new();
    
    println!("Server is listening: {}", server_conn.listening);
    println!("Client is connected: {}", client_conn.connected);
    
    // This is just a simple example to show the new crate structure
    Ok(())
}

// Define a custom channel
struct MyChannel;
impl lightyear::packet::channel::builder::Channel for MyChannel {}
