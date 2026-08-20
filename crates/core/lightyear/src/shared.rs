//! Bevy [`Plugin`] used by both the server and the client
use bevy_app::{App, Plugin};
use core::time::Duration;
use lightyear_core::plugin::CorePlugins;
#[cfg(feature = "replication")]
use lightyear_replication::LightyearRepliconBackend;

pub struct SharedPlugins {
    pub tick_duration: Duration,
}

impl Plugin for SharedPlugins {
    fn build(&self, app: &mut App) {
        // NOTE: this is a clumsy fix to the fact that we cannot control preventing re-adding plugins
        //  when they are a part of a plugin group. See https://github.com/bevyengine/bevy/issues/18909
        if app.is_plugin_added::<CorePlugins>() {
            return;
        }
        app.add_plugins(CorePlugins {
            tick_duration: self.tick_duration,
        })
        .add_plugins(lightyear_transport::plugin::TransportPlugin)
        .add_plugins(lightyear_messages::plugin::MessagePlugin)
        .add_plugins(lightyear_connection::ConnectionPlugin);

        // This private control protocol lives in the shared plugin even though only P2P clients
        // run its lifecycle systems. Message and channel IDs are assigned by registration order,
        // so conventional clients and servers built with `p2p` must reserve the same IDs too.
        #[cfg(feature = "p2p")]
        app.add_plugins(lightyear_p2p::P2PProtocolPlugin);

        #[cfg(feature = "debug")]
        app.add_plugins(lightyear_tools::prelude::LightyearDebugPlugin);

        #[cfg(feature = "replication")]
        app.add_plugins(crate::protocol::ProtocolCheckPlugin);

        #[cfg(feature = "replication")]
        {
            app.add_plugins(LightyearRepliconBackend);
        }

        // Receive-marker registration is part of Replicon's protocol and must
        // be identical on clients and servers before the user's protocol runs.
        #[cfg(all(feature = "prediction", feature = "replication"))]
        app.add_plugins(lightyear_prediction::plugin::PredictionMarkerPlugin);
        #[cfg(all(feature = "interpolation", feature = "replication"))]
        app.add_plugins(lightyear_interpolation::plugin::InterpolationMarkerPlugin);

        // IO
        #[cfg(feature = "crossbeam")]
        app.add_plugins(lightyear_crossbeam::CrossbeamPlugin);
        #[cfg(all(feature = "udp", not(target_family = "wasm")))]
        app.add_plugins(lightyear_udp::UdpPlugin);

        // Note: the server can also do interpolation
        // TODO: move the config to the InterpolationManager
        #[cfg(feature = "interpolation")]
        app.add_plugins(lightyear_interpolation::plugin::InterpolationPlugin);
    }

    fn is_unique(&self) -> bool {
        false
    }
}
