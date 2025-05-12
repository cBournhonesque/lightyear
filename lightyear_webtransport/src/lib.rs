#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

extern crate alloc;

#[cfg(feature = "client")]
pub mod client;
#[cfg(all(feature = "server", not(target_family = "wasm")))]
pub mod server;

pub mod prelude {
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
