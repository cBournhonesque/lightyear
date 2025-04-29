//! This module contains the shared code between the client and the server for the auth example.
use bevy::prelude::*;
use std::net::SocketAddr;

// Define a shared port for the authentication backend
pub const AUTH_BACKEND_PORT: u16 = 4000;

// Resource to store the authentication backend address
#[derive(Resource, Clone, Debug)]
pub struct AuthSettings {
    pub backend_addr: SocketAddr,
}


// // SharedPlugin is no longer needed, ProtocolPlugin is added in main.rs
// #[derive(Clone)]
// pub struct SharedPlugin;
//
// impl Plugin for SharedPlugin {
//     fn build(&self, app: &mut App) {
//         // the protocol needs to be shared between the client and server
//         app.add_plugins(ProtocolPlugin);
//     }
// }
