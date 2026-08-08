use crate::ping::PingChannel;
use crate::ping::manager::PingManager;
use crate::ping::message::{Ping, Pong};
use bevy_app::{App, Plugin, PostUpdate, PreUpdate};
use bevy_ecs::prelude::*;
use bevy_time::{Real, Time};
use core::time::Duration;
#[cfg(feature = "client")]
use lightyear_connection::client::Client;
use lightyear_connection::client::Connected;
use lightyear_connection::direction::NetworkDirection;
use lightyear_connection::host::HostClient;
use lightyear_core::tick::TickDuration;
use lightyear_core::time::Instant;
use lightyear_core::time::TickDelta;
use lightyear_link::Link;
use lightyear_messages::plugin::MessageSystems;
use lightyear_messages::prelude::AppMessageExt;
use lightyear_messages::receive::MessageReceiver;
use lightyear_messages::send::MessageSender;
use lightyear_transport::prelude::{AppChannelExt, ChannelMode, ChannelSettings};
use lightyear_utils::adaptive_for_each_mut;
#[allow(unused_imports)]
use tracing::{info, trace};

#[deprecated(note = "Use PingSystems instead")]
pub type PingSet = PingSystems;

#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone, Copy)]
pub enum PingSystems {
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
        adaptive_for_each_mut!(query).for_each(
            |(mut link, mut m, mut ping_receiver, mut pong_receiver)| {
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
            },
        )
    }

    /// Send pings/pongs to the remote
    /// We modify the pongs that were buffered so that we can write the correct
    /// time spent between PostUpdate and PreUpdate
    fn send(
        tick_duration: Res<TickDuration>,
        mut query: Query<
            (
                Entity,
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
        adaptive_for_each_mut!(query).for_each(
            |(entity, mut m, mut ping_sender, mut pong_sender)| {
                // send the pings
                if let Some(ping) = m.maybe_prepare_ping(now) {
                    trace!(
                        target: "lightyear_debug::sync",
                        kind = "ping_message_send",
                        schedule = "PostUpdate",
                        sample_point = "PostUpdate",
                        entity = ?entity,
                        ping_id = ping.id.0,
                        "queued ping message"
                    );
                    ping_sender.send::<PingChannel>(ping);
                }
                // prepare the pong messages with the correct send time
                m.drain_pending_pongs()
                    .for_each(|(mut pong, ping_receive_time)| {
                        pong.frame_time =
                            TickDelta::from_duration(now - ping_receive_time, tick_duration.0)
                                .into();
                        trace!(?now, ?ping_receive_time, ?pong, "Sending pong");
                        trace!(
                            target: "lightyear_debug::sync",
                            kind = "pong_send",
                            schedule = "PostUpdate",
                            sample_point = "PostUpdate",
                            entity = ?entity,
                            ping_id = pong.ping_id.0,
                            frame_time_ticks = ?pong.frame_time,
                            "queued pong message"
                        );

                        // TODO: maybe include the tick + overstep in every packet?
                        // TODO: how to use the overstep?
                        // pong.overstep = fixed_time.overstep_fraction();
                        pong_sender.send::<PingChannel>(pong);
                    });
            },
        )
    }

    /// On connection, reset the PingManager and restore its typed receivers.
    ///
    /// Disconnection cleanup removes typed receivers so stale messages cannot cross connection
    /// epochs. They are normally recreated lazily by inbound traffic, but two symmetric raw P2P
    /// clients would otherwise both wait for the other side to send the first ping. Restoring the
    /// two protocol-internal receivers here lets either side initiate synchronization.
    pub(crate) fn handle_connect(
        trigger: On<Add, Connected>,
        mut query: Query<&mut PingManager>,
        mut commands: Commands,
    ) {
        if let Ok(mut manager) = query.get_mut(trigger.entity) {
            manager.reset();
            commands
                .entity(trigger.entity)
                .insert_if_new(MessageReceiver::<Ping>::default())
                .insert_if_new(MessageReceiver::<Pong>::default());
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
            ..Default::default()
        })
        .add_direction(NetworkDirection::Bidirectional);
        app.register_message_to_bytes::<Ping>()
            .add_direction(NetworkDirection::Bidirectional);
        app.register_message_to_bytes::<Pong>()
            .add_direction(NetworkDirection::Bidirectional);

        // NOTE: LinkStats are derived from explicit Ping/Pong messages, not transport packet ACKs.
        // Traffic packet ACKs are piggybacked on the peer's next outbound packet; if the peer has
        // nothing to send on the frame it receives a packet, the ACK is delayed and the measured
        // RTT includes that wait. Keep ACK timing as transport telemetry, not canonical RTT.

        // We used to have Client -> LocalTimelineSync -> PingManager -> MessageSender<Ping> -> MessageManager -> Transport -> [Link, LocalTimeline]
        // but it is not possible anymore since we also have a Transport -> PingManager dependency and cyclic dependencies are not allowed anymore.
        //
        // So we removed the Transport -> PingManager dependency. The ping protocol instead owns
        // the requirement for the two connection-entity types on which it runs.
        // app.register_required_components::<Transport, PingManager>();

        #[cfg(feature = "client")]
        app.register_required_components::<Client, PingManager>();

        #[cfg(feature = "server")]
        app.register_required_components::<lightyear_connection::prelude::server::ClientOf, PingManager>();

        app.configure_sets(
            PreUpdate,
            (MessageSystems::Receive, PingSystems::Receive).chain(),
        );
        app.configure_sets(
            PostUpdate,
            (PingSystems::Send, MessageSystems::Send).chain(),
        );
        app.add_systems(PreUpdate, Self::receive.in_set(PingSystems::Receive));
        app.add_systems(PostUpdate, Self::send.in_set(PingSystems::Send));

        app.add_observer(Self::handle_connect);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lightyear_core::id::{PeerId, RemoteId};
    use lightyear_transport::plugin::PacketAcked;

    #[test]
    fn traffic_packet_ack_bursts_do_not_update_ping_rtt() {
        let mut app = App::new();
        app.add_plugins(PingPlugin);

        let entity = app.world_mut().spawn(PingManager::default()).id();

        // Turn-heavy gameplay can acknowledge many ordinary transport packets. These ACK RTT
        // samples are intentionally ignored by the ping estimator; only explicit Ping/Pong samples
        // should drive `LinkStats`.
        for packet_id in 1..=120 {
            app.world_mut().trigger(PacketAcked {
                entity,
                packet_id,
                rtt_sample: Duration::from_millis(50 + u64::from(packet_id)),
            });
        }

        let manager = app.world().entity(entity).get::<PingManager>().unwrap();
        assert_eq!(manager.pongs_recv, 0);
        assert_eq!(manager.latency_samples_recv(), 0);
        assert_eq!(manager.rtt(), Duration::ZERO);
    }

    #[test]
    fn connecting_restores_ping_receivers() {
        let mut app = App::new();
        app.add_plugins(PingPlugin);

        let entity = app
            .world_mut()
            .spawn((RemoteId(PeerId::Server), PingManager::default()))
            .id();
        app.world_mut()
            .entity_mut(entity)
            .remove::<(MessageReceiver<Ping>, MessageReceiver<Pong>)>();

        app.world_mut().entity_mut(entity).insert(Connected);
        app.world_mut().flush();

        let entity = app.world().entity(entity);
        assert!(entity.contains::<MessageReceiver<Ping>>());
        assert!(entity.contains::<MessageReceiver<Pong>>());
    }
}
