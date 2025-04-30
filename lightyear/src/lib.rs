#[cfg(feature = "client")]
mod client;

#[cfg(feature = "server")]
mod server;
mod shared;

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


#[cfg(any(feature = "input_native", feature = "leafwing"))]
pub mod input {
    pub use lightyear_inputs::*;
    #[cfg(feature = "input_native")]
    pub mod native {
        pub use lightyear_inputs_native::*;
    }
}


pub mod connection {
    pub use lightyear_connection::*;
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

    #[cfg(all(not(target_family = "wasm"), feature="udp"))]
    pub use lightyear_udp::prelude::*;

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
    }


    #[cfg(feature = "client")]
    pub mod client {
        pub use crate::client::ClientPlugins;
        pub use lightyear_connection::prelude::client::*;
        pub use lightyear_sync::prelude::client::*;

        #[cfg(feature = "netcode")]
        pub use lightyear_netcode::prelude::client::*;

        #[cfg(any(feature = "input_native", feature = "leafwing"))]
        pub mod input {
            pub use lightyear_inputs::prelude::client::*;
        }

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

        #[cfg(any(feature = "input_native", feature = "leafwing"))]
        pub mod input {
            pub use lightyear_inputs::prelude::server::*;
        }
    }
}