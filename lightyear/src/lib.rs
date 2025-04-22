#[cfg(feature = "client")]
mod client;

#[cfg(feature = "server")]
mod server;
mod shared;

#[cfg(feature = "netcode")]
pub mod netcode {
    pub use lightyear_netcode::*;
}

#[cfg(feature = "prediction")]
pub mod prediction {
    pub use lightyear_prediction::*;
}

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

        pub mod input {
            pub use lightyear_inputs::prelude::client::*;
            #[cfg(feature = "input_native")]
            pub mod native {
                pub use lightyear_inputs_native::prelude::client::*;
            }
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

        pub mod input {
            pub use lightyear_inputs::prelude::server::*;
            #[cfg(feature = "input_native")]
            pub mod native {
                pub use lightyear_inputs_native::prelude::server::*;
            }
        }
    }
}