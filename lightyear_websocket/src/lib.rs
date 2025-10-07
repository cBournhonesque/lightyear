#![cfg_attr(docsrs, feature(doc_cfg))]

extern crate alloc;

#[cfg(feature = "client")]
pub mod client;
#[cfg(all(feature = "server", not(target_family = "wasm")))]
pub mod server;

use alloc::string::String;

#[derive(thiserror::Error, Debug)]
pub enum WebSocketError {
    #[error("the certificate hash `{0}` is invalid")]
    Certificate(String),
    #[error("PeerAddr is required to start the WebSocketClientIo link")]
    PeerAddrMissing,
    #[error("LocalAddr is required to start the WebSocketServerIo")]
    LocalAddrMissing,
}

pub mod prelude {
    pub use crate::WebSocketError;
    pub use aeronet_websocket::*;

    #[cfg(feature = "client")]
    pub mod client {
        pub use crate::client::WebSocketClientIo;
        pub use aeronet_websocket::client::ClientConfig;
    }

    #[cfg(all(feature = "server", not(target_family = "wasm")))]
    pub mod server {
        pub use crate::server::WebSocketServerIo;
        pub use aeronet_websocket::server::ServerConfig;
    }
}
