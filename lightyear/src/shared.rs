//! Bevy [`Plugin`] used by both the server and the client
use bevy::app::PluginGroupBuilder;
use bevy::prelude::*;
use core::time::Duration;
use lightyear_core::plugin::CorePlugins;

pub struct SharedPlugins {
    pub tick_duration: Duration,
}

impl PluginGroup for SharedPlugins {
    fn build(self) -> PluginGroupBuilder {
        let builder = PluginGroupBuilder::start::<Self>();
        let builder = builder
            .add(CorePlugins {
                tick_duration: self.tick_duration,
            })
            .add(lightyear_transport::plugin::TransportPlugin)
            .add(lightyear_messages::plugin::MessagePlugin)
            .add(lightyear_connection::ConnectionPlugin);

        #[cfg(feature = "replication")]
        let builder = builder
            .add(lightyear_replication::prelude::ReplicationSendPlugin)
            .add(lightyear_replication::prelude::NetworkVisibilityPlugin)
            // TODO: this is dangerous because every registered message/component/etc.
            //  needs to be registered at the same time on client/server to guarantee that
            //  they shared the same network_id!
            .add(lightyear_replication::prelude::RelationshipSendPlugin::<
                ChildOf,
            >::default())
            .add(lightyear_replication::prelude::RelationshipReceivePlugin::<
                ChildOf,
            >::default())
            .add(lightyear_replication::prelude::HierarchySendPlugin)
            .add(lightyear_replication::prelude::AuthorityPlugin)
            .add(lightyear_replication::prelude::ReplicationReceivePlugin);

        // IO
        #[cfg(feature = "crossbeam")]
        let builder = builder.add(lightyear_crossbeam::CrossbeamPlugin);
        #[cfg(all(feature = "udp", not(target_family = "wasm")))]
        let builder = builder.add(lightyear_udp::UdpPlugin);

        // Note: the server can also do interpolation
        // TODO: move the config to the InterpolationManager
        #[cfg(feature = "interpolation")]
        let builder = builder.add(lightyear_interpolation::plugin::InterpolationPlugin::new(
            lightyear_interpolation::plugin::InterpolationConfig::default(),
        ));

        #[cfg(feature = "avian")]
        let builder = builder.add(lightyear_avian::LightyearAvianPlugin);

        builder
    }
}
