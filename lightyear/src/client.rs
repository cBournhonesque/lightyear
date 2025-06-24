use bevy_app::{PluginGroup, PluginGroupBuilder};

use crate::shared::SharedPlugins;
use core::time::Duration;

/// A plugin group containing all the client plugins.
///
/// By default, the following plugins will be added:
///
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

        #[cfg(feature = "prediction")]
        let builder = builder.add(lightyear_prediction::plugin::PredictionPlugin);

        // IO
        #[cfg(feature = "webtransport")]
        let builder = builder.add(lightyear_webtransport::client::WebTransportClientPlugin);

        // CONNECTION
        #[cfg(feature = "netcode")]
        let builder = builder.add(lightyear_netcode::client_plugin::NetcodeClientPlugin);

        #[cfg(target_family = "wasm")]
        let builder = builder.add(crate::web::WebPlugin);

        builder
    }
}
