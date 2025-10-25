/*! # Lightyear Raw Connection
Lightyear separates between the IO layer (Link) which handles transmitting bytes to a remote peer,
and the Connection layer  which symbolizes a more long-term connection on top of a Link.

In particular every Connection entity must be associated with a LocalId and RemoteId that identifies
the peers independently of the underlying Link.

This crates provide a connection implementation where the establishing the Link is equivalent to establishing the Connection.
The LocalId and RemoteId come from the Link's SocketAddr.
*/
#![no_std]

extern crate alloc;
extern crate core;

#[cfg(feature = "client")]
pub mod client;

#[cfg(feature = "server")]
pub mod server;

pub mod prelude {
    #[cfg(feature = "client")]
    pub mod client {
        pub use crate::client::RawClient;
    }

    #[cfg(feature = "server")]
    pub mod server {
        pub use crate::server::RawServer;
    }
}
