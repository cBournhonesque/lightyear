

pub mod prelude {
    pub use lightyear_core::prelude::*;
    pub use lightyear_link::prelude::*;
    pub use lightyear_messages::prelude::*;
    pub use lightyear_sync::prelude::*;
    pub use lightyear_transport::prelude::*;

    #[cfg(any(feature = "client", feature = "server"))]
    pub use lightyear_connection::prelude::*;

    #[cfg(feature = "client")]
    pub mod client {
        pub use lightyear_client::plugin::ClientPlugins;
        pub use lightyear_connection::prelude::client::*;
        pub use lightyear_sync::prelude::client::*;
    }

    #[cfg(feature = "server")]
    pub mod server {
        pub use lightyear_connection::prelude::server::*;
        pub use lightyear_server::plugin::ServerPlugins;
        pub use lightyear_sync::prelude::server::*;
    }
}