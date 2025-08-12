//! Bevy [`Plugin`] used by both the server and the client
use bevy_app::{App, Plugin};
use bevy_ecs::prelude::ChildOf;
use core::time::Duration;
use lightyear_core::plugin::CorePlugins;

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

        #[cfg(feature = "replication")]
        app.add_plugins(crate::protocol::ProtocolCheckPlugin);

        #[cfg(feature = "replication")]
        app.add_plugins(lightyear_replication::prelude::ReplicationSendPlugin)
            .add_plugins(lightyear_replication::prelude::NetworkVisibilityPlugin)
            .add_plugins(lightyear_replication::prelude::HierarchySendPlugin::<ChildOf>::default())
            .add_plugins(lightyear_replication::prelude::AuthorityPlugin)
            .add_plugins(lightyear_replication::prelude::ReplicationReceivePlugin);

        // IO
        #[cfg(feature = "crossbeam")]
        app.add_plugins(lightyear_crossbeam::CrossbeamPlugin);
        #[cfg(all(feature = "udp", not(target_family = "wasm")))]
        app.add_plugins(lightyear_udp::UdpPlugin);

        // Note: the server can also do interpolation
        // TODO: move the config to the InterpolationManager
        #[cfg(feature = "interpolation")]
        app.add_plugins(lightyear_interpolation::plugin::InterpolationPlugin);

        #[cfg(feature = "avian2d")]
        app.add_plugins(lightyear_avian2d::prelude::LightyearAvianPlugin::default());
        #[cfg(feature = "avian3d")]
        {
            app.add_plugins(lightyear_avian3d::prelude::LightyearAvianPlugin::default());
        }
    }

    fn is_unique(&self) -> bool {
        false
    }
}
