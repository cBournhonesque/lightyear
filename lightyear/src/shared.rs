//! Bevy [`Plugin`] used by both the server and the client
use bevy::prelude::*;
use core::time::Duration;
use lightyear_core::plugin::CorePlugins;
use lightyear_core::timeline::SetTickDuration;
// NOTE: we cannot use nested PluginGroups so let's just put everything in a plugin
// #[derive(Default, Debug)]
// pub struct SharedPlugins {
//     tick_duration: Duration
// }
//
// impl PluginGroup for SharedPlugins {
//     fn build(self) -> PluginGroupBuilder {
//         let builder = PluginGroupBuilder::start::<Self>();
//         let builder = builder
//             .add(SetupPlugin {
//                 tick_duration: self.tick_duration
//             })
//             .add(lightyear_transport::plugin::TransportPlugin)
//             .add(lightyear_messages::plugin::MessagePlugin)
//             .add(lightyear_core::time::TimePlugin);
//         builder
//     }
// }

pub struct SharedPlugin{
    pub tick_duration: Duration
}


impl Plugin for SharedPlugin {
    fn build(&self, app: &mut App) {
        app
            .add_plugins(CorePlugins {
                tick_duration: self.tick_duration,
            })
            .add_plugins(lightyear_transport::plugin::TransportPlugin)
            .add_plugins(lightyear_messages::plugin::MessagePlugin)
            // TODO: make this a plugin group so it's possible to disable plugins
            .add_plugins(lightyear_replication::prelude::ReplicationSendPlugin)
            .add_plugins(lightyear_replication::prelude::RelationshipSendPlugin::<ChildOf>::default())
            .add_plugins(lightyear_replication::prelude::RelationshipReceivePlugin::<ChildOf>::default())
            .add_plugins(lightyear_replication::prelude::HierarchySendPlugin)
            .add_plugins(lightyear_replication::prelude::ReplicationReceivePlugin);

        // IO
        #[cfg(feature = "crossbeam")]
        app.add_plugins(lightyear_crossbeam::CrossbeamPlugin);
        #[cfg(feature = "udp")]
        app.add_plugins(lightyear_udp::UdpPlugin);
    }

    /// After timelines and PingManager are created, trigger a TickDuration event
    fn finish(&self, app: &mut App) {
        app.world_mut().trigger(SetTickDuration(self.tick_duration));
    }
}
