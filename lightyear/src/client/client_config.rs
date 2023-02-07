use std::{default::Default, time::Duration};

use crate::shared::ConnectionConfig;

/// Contains Config properties which will be used by a Server or Client
#[derive(Clone)]
pub struct ClientConfig {
    /// Used to configure the connection with the Server
    pub connection: ConnectionConfig,
    /// The duration between the resend of certain connection handshake messages
    pub send_handshake_interval: Duration,
    /// The minimum of measured latency to the Server that the Client use to
    /// ensure packets arrive in time. Should be fine if this is 0,
    /// but you'll increase the chance that packets always arrive to be
    /// processed by the Server with a higher number. This is especially
    /// helpful early on in the connection, when estimates of latency are
    /// less accurate.
    pub minimum_latency: Option<Duration>,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            connection: ConnectionConfig::default(),
            send_handshake_interval: Duration::from_millis(250),
            minimum_latency: None,
        }
    }
}
