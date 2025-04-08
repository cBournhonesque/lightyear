//! Bevy [`Plugin`] used by both the server and the client
use bevy::prelude::*;
use core::time::Duration;
use lightyear_connection::prelude::*;
use lightyear_core::time::SetTickDuration;
use lightyear_messages::prelude::*;
use lightyear_sync::prelude::*;
use lightyear_transport::prelude::*;

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

impl SharedPlugin {
    fn add_channels(app: &mut App) {
        app.add_channel::<PingChannel>(ChannelSettings {
                           mode: ChannelMode::SequencedUnreliable,
                           send_frequency: Duration::default(),
                           // we always want to include the ping in the packet
                           priority: f32::INFINITY,
                       })
            .add_direction(NetworkDirection::Bidirectional);
    }

    fn add_messages(app: &mut App) {
        app.add_message_to_bytes::<Ping>()
            .add_direction(NetworkDirection::Bidirectional);
        app.add_message_to_bytes::<Pong>()
            .add_direction(NetworkDirection::Bidirectional);
    }
}

impl Plugin for SharedPlugin {
    fn build(&self, app: &mut App) {
        app
            .add_plugins(lightyear_transport::plugin::TransportPlugin)
            .add_plugins(lightyear_messages::plugin::MessagePlugin)
            .add_plugins(lightyear_core::tick::TickPlugin {
                tick_duration: self.tick_duration
            })
            .add_plugins(lightyear_core::time::TimePlugin)
            // TODO: make this a plugin group so it's possible to disable plugins
            .add_plugins(lightyear_replication::prelude::ReplicationSendPlugin)
            .add_plugins(lightyear_replication::prelude::ReplicationReceivePlugin);
        ;

        Self::add_channels(app);
        Self::add_messages(app);


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
