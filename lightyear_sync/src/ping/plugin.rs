use crate::ping::manager::PingManager;
use crate::ping::message::{Ping, Pong};
use crate::ping::PingChannel;
use bevy::prelude::*;
use core::time::Duration;
use lightyear_connection::client::Connected;
use lightyear_connection::direction::NetworkDirection;
use lightyear_connection::host::HostClient;
use lightyear_core::tick::TickDuration;
use lightyear_core::time::Instant;
use lightyear_core::time::TickDelta;
use lightyear_link::Link;
use lightyear_messages::plugin::MessageSet;
use lightyear_messages::prelude::AppMessageExt;
use lightyear_messages::receive::MessageReceiver;
use lightyear_messages::send::MessageSender;
use lightyear_transport::prelude::{AppChannelExt, ChannelMode, ChannelSettings, Transport};

#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone, Copy)]
pub enum PingSet {
    /// Receive messages from the Link and buffer them into the ChannelReceivers
    Receive,
    /// Flush the messages buffered in the ChannelSenders to the Link
    Send,
}

pub struct PingPlugin;

impl PingPlugin {
    fn receive(
        real_time: Res<Time<Real>>,
        tick_duration: Res<TickDuration>,
        mut query: Query<
            (
                &mut Link,
                &mut PingManager,
                &mut MessageReceiver<Ping>,
                &mut MessageReceiver<Pong>,
            ),
            (With<Connected>, Without<HostClient>),
        >,
    ) {
        query
            .par_iter_mut()
            .for_each(|(mut link, mut m, mut ping_receiver, mut pong_receiver)| {
                // update
                m.update(&real_time);

                // receive pings
                ping_receiver.receive().for_each(|ping| {
                    m.buffer_pending_pong(&ping, Instant::now());
                });
                // receive pongs
                pong_receiver.receive().for_each(|pong| {
                    // process the pong
                    m.process_pong(&pong, Instant::now(), tick_duration.0);
                });

                link.stats.rtt = m.rtt();
                link.stats.jitter = m.jitter();
            })
    }

    /// Send pings/pongs to the remote
    /// We modify the pongs that were buffered so that we can write the correct
    /// time spent between PostUpdate and PreUpdate
    fn send(
        tick_duration: Res<TickDuration>,
        mut query: Query<
            (
                &mut PingManager,
                &mut MessageSender<Ping>,
                &mut MessageSender<Pong>,
            ),
            (With<Connected>, Without<HostClient>),
        >,
    ) {
        let now = Instant::now();
        // NOTE: the real_time.last_update() is the time from the Render World! It seems like it cannot be compared directly
        //  with the time from Instant::now(), so we stick to only using Instant::now() for now.
        // let Some(frame_start) = real_time.last_update() else {
        //     return
        // };
        // let frame_time = now - frame_start;
        query
            .par_iter_mut()
            .for_each(|(mut m, mut ping_sender, mut pong_sender)| {
                // send the pings
                if let Some(ping) = m.maybe_prepare_ping(Instant::now()) {
                    ping_sender.send::<PingChannel>(ping);
                }
                // prepare the pong messages with the correct send time
                m.take_pending_pongs()
                    .into_iter()
                    .for_each(|(mut pong, ping_receive_time)| {
                        pong.frame_time =
                            TickDelta::from_duration(now - ping_receive_time, tick_duration.0)
                                .into();
                        trace!(?now, ?ping_receive_time, ?pong, "Sending pong");

                        // TODO: maybe include the tick + overstep in every packet?
                        // TODO: how to use the overstep?
                        // pong.overstep = fixed_time.overstep_fraction();
                        pong_sender.send::<PingChannel>(pong);
                    });
            })
    }

    /// On connection, reset the PingManager.
    pub(crate) fn handle_connect(
        trigger: Trigger<OnAdd, Connected>,
        mut query: Query<&mut PingManager>,
    ) {
        if let Ok(mut manager) = query.get_mut(trigger.target()) {
            manager.reset();
        }
    }
}

impl Plugin for PingPlugin {
    fn build(&self, app: &mut App) {
        app.add_channel::<PingChannel>(ChannelSettings {
            // NOTE: using Sequenced is invalid if we are sharing a channel between Ping and Pong!
            mode: ChannelMode::UnorderedUnreliable,
            send_frequency: Duration::default(),
            // we always want to include the ping in the packet
            priority: f32::INFINITY,
        })
        .add_direction(NetworkDirection::Bidirectional);
        app.add_message_to_bytes::<Ping>()
            .add_direction(NetworkDirection::Bidirectional);
        app.add_message_to_bytes::<Pong>()
            .add_direction(NetworkDirection::Bidirectional);

        // NOTE: the Transport's PacketBuilder needs accurate LinkStats to function correctly.
        //   Theoretically anything can modify the LinkStats but in practice it's done in the PingManager
        //   so we make the Transport require a PingManager.
        //   Maybe we should error if TransportPlugin is added without PingPlugin?
        app.register_required_components::<Transport, PingManager>();

        #[cfg(feature = "server")]
        app.register_required_components::<lightyear_connection::prelude::server::ClientOf, PingManager>();

        app.configure_sets(PreUpdate, (MessageSet::Receive, PingSet::Receive).chain());
        app.configure_sets(PostUpdate, (PingSet::Send, MessageSet::Send).chain());
        app.add_systems(PreUpdate, Self::receive.in_set(PingSet::Receive));
        app.add_systems(PostUpdate, Self::send.in_set(PingSet::Send));

        app.add_observer(Self::handle_connect);
    }
}
