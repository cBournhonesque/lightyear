use std::default::Default;

use crate::shared::ConnectionConfig;

/// Contains Config properties which will be used by the Server
#[derive(Clone)]
pub struct ServerConfig {
    /// Used to configure the connections with Clients
    pub connection: ConnectionConfig,
    /// Determines whether to require that the Client send some auth message
    /// in order to connect.
    pub require_auth: bool,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            connection: ConnectionConfig::default(),
            require_auth: true,
        }
    }
}
