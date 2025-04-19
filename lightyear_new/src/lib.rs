#[cfg(feature = "client")]
mod client;

#[cfg(feature = "server")]
mod server;
mod shared;

#[cfg(feature = "netcode")]
pub mod netcode {
    pub use lightyear_netcode::*;
}

pub mod connection {
    pub use lightyear_connection::*;
}

pub mod prelude {
    pub use lightyear_connection::prelude::*;
    pub use lightyear_core::prelude::*;
    pub use lightyear_link::prelude::*;
    pub use lightyear_messages::prelude::*;
    pub use lightyear_replication::prelude::*;
    pub use lightyear_sync::prelude::*;
    pub use lightyear_transport::prelude::*;

    #[cfg(all(not(target_family = "wasm"), feature="udp"))]
    pub use lightyear_udp::prelude::*;

    #[cfg(feature = "netcode")]
    pub use lightyear_netcode::prelude::*;


    #[cfg(feature = "client")]
    pub mod client {
        pub use crate::client::ClientPlugins;
        pub use lightyear_connection::prelude::client::*;
        pub use lightyear_sync::prelude::client::*;

        #[cfg(feature = "netcode")]
        pub use lightyear_netcode::prelude::client::*;
    }

    #[cfg(feature = "server")]
    pub mod server {
        pub use crate::server::ServerPlugins;
        pub use lightyear_connection::prelude::server::*;
        pub use lightyear_sync::prelude::server::*;

        #[cfg(all(not(target_family = "wasm"), feature = "udp", feature = "server"))]
        pub use lightyear_udp::prelude::server::*;


        #[cfg(feature = "netcode")]
        pub use lightyear_netcode::prelude::server::*;
    }
}