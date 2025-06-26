#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

extern crate alloc;

#[cfg(feature = "client")]
pub mod client;
#[cfg(all(feature = "server", not(target_family = "wasm")))]
pub mod server;

use alloc::string::String;

#[derive(thiserror::Error, Debug)]
pub enum WebTransportError {
    #[error("the certificate hash `{0}` is invalid")]
    Certificate(String),
    #[error("PeerAddr is required to start the WebTransportClientIo link")]
    PeerAddrMissing,
    #[error("LocalAddr is required to start the WebTransportServerIo")]
    LocalAddrMissing,
}

pub mod prelude {
    pub use crate::WebTransportError;

    #[cfg(not(target_family = "wasm"))]
    pub use aeronet_webtransport::wtransport::Identity;

    #[cfg(feature = "client")]
    pub mod client {
        pub use crate::client::WebTransportClientIo;
    }

    #[cfg(all(feature = "server", not(target_family = "wasm")))]
    pub mod server {
        pub use crate::server::WebTransportServerIo;
    }
}
