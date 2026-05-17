//! This module contains the shared code between the client and the server for the auth example.
use core::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use core::str::FromStr;

// Define a shared port for the authentication backend
pub const AUTH_BACKEND_PORT: u16 = 4000;

pub const AUTH_BACKEND_ADDRESS: SocketAddr =
    SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, AUTH_BACKEND_PORT));

pub fn auth_backend_address() -> SocketAddr {
    std::env::var("LIGHTYEAR_AUTH_BACKEND_ADDRESS")
        .ok()
        .and_then(|value| SocketAddr::from_str(&value).ok())
        .or_else(|| {
            std::env::var("LIGHTYEAR_AUTH_BACKEND_PORT")
                .ok()
                .and_then(|value| value.parse::<u16>().ok())
                .map(|port| SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, port)))
        })
        .unwrap_or(AUTH_BACKEND_ADDRESS)
}
