use bevy_app::{PluginGroup, PluginGroupBuilder};

use crate::shared::SharedPlugins;
use core::time::Duration;

/// A plugin group containing all the client plugins.
///
/// The order in which the plugins are added matters!
/// You need to add:
/// - first add the `ClientPlugins`
/// - then build your protocol (usually in a `ProtocolPlugin`)
/// - then spawn your `Client` entity
pub struct ClientPlugins {
    /// The tick interval for the client. This is used to determine how often the client should tick.
    /// The default value is 1/60 seconds.
    pub tick_duration: Duration,
}

impl PluginGroup for ClientPlugins {
    #[allow(clippy::let_and_return)]
    fn build(self) -> PluginGroupBuilder {
        let builder = PluginGroupBuilder::start::<Self>();

        let builder = builder.add(lightyear_sync::client::ClientPlugin);

        let builder = builder.add(SharedPlugins {
            tick_duration: self.tick_duration,
        });

        // if the server feature is enabled (e.g. for host-server mode), then we don't need
        // the client to send checksum messages
        #[cfg(all(feature = "deterministic", not(feature = "server")))]
        let builder = builder.add(lightyear_deterministic_replication::prelude::ChecksumSendPlugin);

        #[cfg(feature = "prediction")]
        let builder = builder.add(lightyear_prediction::plugin::PredictionPlugin);

        // IO
        #[cfg(feature = "webtransport")]
        let builder = builder.add(lightyear_webtransport::client::WebTransportClientPlugin);
        #[cfg(feature = "steam")]
        let builder = builder.add(lightyear_steam::client::SteamClientPlugin);

        // CONNECTION
        #[cfg(feature = "netcode")]
        let builder = builder.add(lightyear_netcode::client_plugin::NetcodeClientPlugin);

        #[cfg(target_family = "wasm")]
        let builder = builder.add(crate::web::WebPlugin);

        builder
    }
}
