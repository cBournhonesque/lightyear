//! This module contains the shared code between the client and the server for the auth example.
use core::net::{Ipv4Addr, SocketAddr, SocketAddrV4};

// Define a shared port for the authentication backend
pub const AUTH_BACKEND_PORT: u16 = 4000;

pub const AUTH_BACKEND_ADDRESS: SocketAddr =
    SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, AUTH_BACKEND_PORT));
