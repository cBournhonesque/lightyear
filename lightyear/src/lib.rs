#![allow(ambiguous_glob_reexports)]

#[cfg(feature = "client")]
pub mod client;

#[cfg(all(feature = "server", not(target_family = "wasm")))]
mod server;
mod shared;

#[cfg(target_family = "wasm")]
mod web;

#[cfg(feature = "netcode")]
pub mod netcode {
    pub use lightyear_netcode::*;
}

#[cfg(feature = "interpolation")]
pub mod interpolation {
    pub use lightyear_interpolation::*;
}

#[cfg(feature = "prediction")]
pub mod prediction {
    pub use lightyear_prediction::*;
}

#[cfg(feature = "webtransport")]
pub mod webtransport {
    pub use lightyear_webtransport::*;
}

#[cfg(any(feature = "input_native", feature = "leafwing"))]
pub mod input {
    pub use lightyear_inputs::*;
    #[cfg(feature = "input_native")]
    pub mod native {
        pub use lightyear_inputs_native::*;
    }

    #[cfg(feature = "leafwing")]
    pub mod leafwing {
        pub use lightyear_inputs_leafwing::*;
    }
}

pub mod connection {
    pub use lightyear_connection::*;
}

pub mod utils {
    pub use lightyear_utils::*;
}

pub mod prelude {
    pub use lightyear_connection::prelude::*;
    pub use lightyear_core::prelude::*;
    pub use lightyear_link::prelude::*;
    pub use lightyear_messages::prelude::*;
    #[cfg(feature = "replication")]
    pub use lightyear_replication::prelude::*;
    pub use lightyear_sync::prelude::*;
    pub use lightyear_transport::prelude::*;

    #[cfg(all(not(target_family = "wasm"), feature = "udp"))]
    pub use lightyear_udp::prelude::*;

    #[allow(unused_imports)]
    #[cfg(feature = "webtransport")]
    pub use lightyear_webtransport::prelude::*;

    #[cfg(feature = "netcode")]
    pub use lightyear_netcode::prelude::*;

    // TODO: maybe put this in prelude::client?
    #[cfg(feature = "prediction")]
    pub use lightyear_prediction::prelude::*;

    #[cfg(feature = "interpolation")]
    pub use lightyear_interpolation::prelude::*;

    #[cfg(any(feature = "input_native", feature = "leafwing"))]
    pub mod input {
        pub use lightyear_inputs::prelude::*;
        #[cfg(feature = "input_native")]
        pub mod native {
            pub use lightyear_inputs_native::prelude::*;
        }
        #[cfg(feature = "leafwing")]
        pub mod leafwing {
            pub use lightyear_inputs_leafwing::prelude::*;
        }
    }

    #[cfg(feature = "client")]
    pub mod client {
        pub use crate::client::ClientPlugins;

        pub use lightyear_sync::prelude::client::*;

        #[cfg(feature = "netcode")]
        pub use lightyear_netcode::prelude::client::*;
        #[cfg(feature = "webtransport")]
        pub use lightyear_webtransport::prelude::client::*;

        #[cfg(any(feature = "input_native", feature = "leafwing"))]
        pub mod input {
            pub use lightyear_inputs::prelude::client::*;
        }
    }

    #[cfg(all(feature = "server", not(target_family = "wasm")))]
    pub mod server {
        pub use crate::server::ServerPlugins;
        pub use lightyear_connection::prelude::server::*;
        pub use lightyear_link::prelude::server::*;

        #[cfg(all(not(target_family = "wasm"), feature = "udp", feature = "server"))]
        pub use lightyear_udp::prelude::server::*;

        #[cfg(feature = "netcode")]
        pub use lightyear_netcode::prelude::server::*;
        #[cfg(feature = "webtransport")]
        pub use lightyear_webtransport::prelude::server::*;

        #[cfg(any(feature = "input_native", feature = "leafwing"))]
        pub mod input {
            pub use lightyear_inputs::prelude::server::*;
        }
    }
}
