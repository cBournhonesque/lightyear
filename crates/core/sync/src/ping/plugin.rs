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
        let Some(now) = real_time.last_update() else {
            return;
        };
        adaptive_for_each_mut!(query).for_each(
            |(mut link, mut m, mut ping_receiver, mut pong_receiver)| {
                // update
                m.update(&real_time);

                // receive pings
                ping_receiver.receive().for_each(|ping| {
                    m.buffer_pending_pong(&ping, now);
                });
                // receive pongs
                pong_receiver.receive().for_each(|pong| {
                    // process the pong
                    m.process_pong(&pong, now, tick_duration.0);
                });

                link.stats.rtt = m.rtt();
                link.stats.jitter = m.jitter();
            },
        )
    }

    /// Send pings/pongs using the application's frame timestamp.
    ///
    /// [`Pong::frame_time`] corrects waits across application time updates. Receive and send
    /// normally share a frame timestamp, so intra-frame processing is included in measured RTT.
    fn send(
        real_time: Res<Time<Real>>,
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
        // Use the same clock as ping timers, transport timers and timeline synchronization.
        let Some(now) = real_time.last_update() else {
            return;
        };
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
        trigger: On<Add<Connected>>,
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
    use bevy_time::{TimeUpdateStrategy, Virtual};
    use lightyear_core::id::{PeerId, RemoteId};
    use lightyear_core::plugin::CorePlugins;
    use lightyear_link::{Linked, recv_payload_from_bytes};
    use lightyear_messages::plugin::MessagePlugin;
    use lightyear_transport::prelude::{ChannelRegistry, Transport};
    use lightyear_transport::{packet::PacketId, plugin::PacketAcked};

    #[test]
    fn ping_rtt_follows_manual_real_time() {
        // Cover both normal same-frame replies and pongs held across a clock update.
        for pong_wait in [Duration::ZERO, Duration::from_millis(30)] {
            let mut app = App::new();
            app.add_plugins((
                bevy_app::TaskPoolPlugin::default(),
                CorePlugins {
                    tick_duration: Duration::from_millis(10),
                },
                PingPlugin,
                MessagePlugin,
            ));
            app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs(1)));
            app.finish();
            app.update();
            // Networking must keep using Real time even when gameplay time is paused.
            app.world_mut().resource_mut::<Time<Virtual>>().pause();

            let registry = app.world().resource::<ChannelRegistry>();
            let mut transport = Transport::default();
            transport.add_channel_send_from_registry::<PingChannel>(registry);
            transport.add_channel_receive_from_registry::<PingChannel>(registry);
            let entity = app
                .world_mut()
                .spawn((
                    Link::default(),
                    Linked,
                    transport,
                    RemoteId(PeerId::Server),
                    Connected,
                    PingManager::new(crate::ping::manager::PingConfig {
                        ping_interval: Duration::from_secs(1),
                    }),
                ))
                .id();

            app.update();
            assert_eq!(
                app.world().get::<PingManager>(entity).unwrap().pings_sent,
                1
            );
            loopback_ping_packet(&mut app, entity);

            // Receive the ping after 40 ms of application time, then optionally hold its pong.
            app.world_mut()
                .resource_mut::<Time<Real>>()
                .update_with_duration(Duration::from_millis(40));
            app.world_mut().run_schedule(PreUpdate);
            if !pong_wait.is_zero() {
                app.world_mut()
                    .resource_mut::<Time<Real>>()
                    .update_with_duration(pong_wait);
            }
            app.world_mut().run_schedule(PostUpdate);
            loopback_ping_packet(&mut app, entity);

            app.world_mut()
                .resource_mut::<Time<Real>>()
                .update_with_duration(Duration::from_millis(60));
            app.world_mut().run_schedule(PreUpdate);

            let manager = app.world().get::<PingManager>(entity).unwrap();
            assert_eq!(manager.pongs_recv, 1);
            assert_eq!(manager.latency_samples_recv(), 1);
            // RTT excludes any wait across clock updates: 40 + 60 = 100 ms in both cases.
            assert_eq!(manager.rtt(), Duration::from_millis(100));
            assert_eq!(
                app.world().get::<Link>(entity).unwrap().stats.rtt,
                Duration::from_millis(100)
            );
        }
    }

    fn loopback_ping_packet(app: &mut App, entity: Entity) {
        let mut link = app.world_mut().get_mut::<Link>(entity).unwrap();
        assert_eq!(link.send.len(), 1);
        let packet = link.send.pop().unwrap();
        link.recv.push_raw(recv_payload_from_bytes(packet));
    }

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
                packet_id: PacketId(packet_id),
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
