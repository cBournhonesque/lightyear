//! Real time estimation of the packet RTT (round-trip time) overhead introduced
//! by lightyear. Include [`RttOverheadEstimatorPlugin`] in your app and trigger
//! [`EstimateRttOverhead`] to calculate an estimation. The estimation is
//! published once as an [`RttOverheadEstimated`] event. You can use this as
//! the RTT estimate required by
//! [`LinkConditionerConfig::fit`](lightyear_link::prelude::LinkConditionerConfig::fit).
//!
//! RTT overhead is comprised of:
//!
//! * The delay between lightyear receiving a packet and lightyear sending the
//!   ack for it (the ack is not sent until the next scheduled outgoing packet).
//! * The delay between lightyear receiving an ack and lightyear actually
//!   processing it.
//!
//! To estimate the RTT overhead, the estimator spawns two endpoints (marked
//! with [`EstimatorEndpoint`]), connects them over an in-process
//! [`CrossbeamIo`] pair, and sends packets between them through lightyear until
//! it collects enough packet RTTs. The endpoints are in the same process, so
//! there is no network latency between them, and the RTT overhead is the only
//! thing that can make a packet's RTT greater than zero. The RTT overhead
//! estimate is the median of those packets' RTTs.
//!
//! The overhead is dominated by frame/tick-boundary scheduling, so it reflects
//! the process's current frame rate. Trigger [`EstimateRttOverhead`] once the
//! app is running at its steady-state cadence rather than mid-startup.
//!
//! Most likely you are requesting this estimate in order to pass it into
//! [`fit`](lightyear_link::prelude::LinkConditionerConfig::fit). If that's the
//! case, you will also need to collect the packets sent by a [`Transport`]. You
//! can do this by creating
//! [`ResolvedPacket`](lightyear_link::prelude::ResolvedPacket)s whenever a
//! [`PacketAcked`](crate::plugin::PacketAcked) or
//! [`PacketLost`](crate::plugin::PacketLost) is triggered (both events convert
//! via `Into`). Remember to ignore the [`EstimatorEndpoint`]s when collecting
//! because their packets aren't sent across a real network thus their RTTs are
//! not what you want.
//!
//! ```
//! use std::collections::HashMap;
//!
//! use bevy_ecs::prelude::*;
//! use lightyear_link::prelude::ResolvedPacket;
//! use lightyear_transport::plugin::{PacketAcked, PacketLost};
//! use lightyear_transport::prelude::EstimatorEndpoint;
//!
//! /// Captures packets sent per link.
//! #[derive(Resource, Default)]
//! struct ResolvedPackets(HashMap<Entity, Vec<ResolvedPacket>>);
//!
//! fn record_acked(
//!     trigger: On<PacketAcked>,
//!     estimator_endpoints: Query<(), With<EstimatorEndpoint>>,
//!     mut sent: ResMut<ResolvedPackets>,
//! ) {
//!     let event = trigger.event();
//!     if estimator_endpoints.contains(event.entity) {
//!         // Ignore packets from RTT-overhead-estimator endpoints. Their
//!         // packets aren't sent over a real network.
//!         return;
//!     }
//!     sent.0.entry(event.entity).or_default().push((*event).into());
//! }
//!
//! fn record_lost(
//!     trigger: On<PacketLost>,
//!     estimator_endpoints: Query<(), With<EstimatorEndpoint>>,
//!     mut sent: ResMut<ResolvedPackets>,
//! ) {
//!     let event = trigger.event();
//!     if estimator_endpoints.contains(event.entity) {
//!         // Ignore packets from RTT-overhead-estimator endpoints. Their
//!         // packets aren't sent over a real network.
//!         return;
//!     }
//!     sent.0.entry(event.entity).or_default().push((*event).into());
//! }
//!
//! let mut app = bevy_app::App::new();
//! app.init_resource::<ResolvedPackets>();
//! app.add_observer(record_acked);
//! app.add_observer(record_lost);
//! ```

use alloc::vec::Vec;
use core::time::Duration;

use bevy_app::prelude::*;
use bevy_ecs::prelude::*;
use bytes::Bytes;
use lightyear_crossbeam::{CrossbeamIo, CrossbeamPlugin};
use lightyear_link::{Link, LinkStart};

use crate::{
    channel::{
        builder::{ChannelMode, ChannelSettings, Transport},
        receivers::ChannelReceive,
        registry::{AppChannelExt, ChannelRegistry},
    },
    plugin::PacketAcked,
};

/// Default number of loopback packets that [`EstimateRttOverhead`] will gather
/// RTTs from if not specified otherwise.
const DEFAULT_PACKET_COUNT: usize = 1024;

/// Registers the RTT overhead estimator. You need to add this plugin before
/// triggering [`EstimateRttOverhead`] in order to get an RTT overhead estimate.
pub struct RttOverheadEstimatorPlugin;

impl Plugin for RttOverheadEstimatorPlugin {
    fn build(&self, app: &mut App) {
        app.add_channel::<EstimatorChannel>(ChannelSettings {
            mode: ChannelMode::UnorderedUnreliable,
            ..Default::default()
        });
        if !app.is_plugin_added::<CrossbeamPlugin>() {
            app.add_plugins(CrossbeamPlugin);
        }
        app.add_systems(Update, (send_estimator_traffic, drain_estimator_messages));
        app.add_observer(start_estimation);
        app.add_observer(record_endpoint_rtt);
    }
}

/// Request to estimate the RTT overhead. The estimate will be returned in an
/// [`RttOverheadEstimated`]. Ignored if a estimation is currently underway.
#[derive(Event, Debug, Clone, Copy)]
pub struct EstimateRttOverhead {
    /// How many loopback packets to collect RTTs from before estimating their
    /// median.
    pub packet_count: usize,
}

impl Default for EstimateRttOverhead {
    fn default() -> Self {
        Self {
            packet_count: DEFAULT_PACKET_COUNT,
        }
    }
}

/// Estimation of the per-packet RTT overhead. Fired once per completed
/// [`EstimateRttOverhead`] request.
///
/// Pass the carried [`Duration`] to
/// [`LinkConditionerConfig::fit`](lightyear_link::prelude::LinkConditionerConfig::fit).
#[derive(Event, Debug, Clone, Copy)]
pub struct RttOverheadEstimated(pub Duration);

/// The channel that carries the RTT overhead estimator's dummy traffic in order
/// to generate packet RTTs to feed into the RTT overhead estimate.
#[derive(Debug)]
struct EstimatorChannel;

/// Marks a loopback [`Link`] used by the RTT overhead estimator to send packets
/// and record their RTTs.
///
/// Public so packet-capture consumers can exclude these links: their traffic
/// never crosses a network, so their [`PacketAcked`] RTTs measure only
/// lightyear's own overhead.
#[derive(Component, Debug)]
pub struct EstimatorEndpoint;

/// State information about an RTT estimation in progress. Only exists if there
/// is an estimation in progress.
#[derive(Resource)]
struct RttOverheadState {
    /// Number of loopback packets to gather RTTs from before estimating their
    /// median.
    packet_count: usize,

    /// RTTs of the packets sent by the [`EstimatorEndpoint`]s.
    packet_rtts: Vec<Duration>,
}

/// On [`EstimateRttOverhead`], starts an RTT over head estimation. Ignores the
/// request if one is already running.
fn start_estimation(
    trigger: On<EstimateRttOverhead>,
    registry: Res<ChannelRegistry>,
    running: Option<Res<RttOverheadState>>,
    mut commands: Commands,
) {
    if running.is_some() {
        // Estimation already in progress.
        return;
    }

    let packet_count = trigger.event().packet_count;
    commands.insert_resource(RttOverheadState {
        packet_count,
        packet_rtts: Vec::with_capacity(packet_count),
    });

    // Create the endpoints that send the dummy data.
    let transport = || {
        let mut transport = Transport::default();
        transport.add_sender_from_registry::<EstimatorChannel>(&registry);
        transport.add_receiver_from_registry::<EstimatorChannel>(&registry);
        transport
    };
    let seeded_link = || {
        let mut link = Link::new(None);

        // Set to 1 second so packets are not declared lost before their acks have
        // arrived. The transport computes its packet-loss timeout from this field,
        // and nothing on these links updates the field (no PingManager runs here), so
        // the timeout would have stayed at 10ms while the loopback ack takes about two
        // frames worth of wall time.
        //
        // `LinkStats::rtt` is not the RTT overhead being estimated, and the
        // estimate never reads it.
        link.stats.rtt = Duration::from_secs(1);
        link
    };
    let (io_a, io_b) = CrossbeamIo::new_pair();
    for io in [io_a, io_b] {
        let endpoint = commands
            .spawn((transport(), io, seeded_link(), EstimatorEndpoint))
            .id();
        commands.trigger(LinkStart { entity: endpoint });
    }
}

/// Records the RTT of packets sent by the [`EstimatorEndpoint`]s that were
/// acked. Fires [`RttOverheadEstimated`] if there are enough RTTs to calculate
/// the estimate.
fn record_endpoint_rtt(
    trigger: On<PacketAcked>,
    endpoints: Query<Entity, With<EstimatorEndpoint>>,
    state: Option<ResMut<RttOverheadState>>,
    mut commands: Commands,
) {
    let Some(mut state) = state else {
        return;
    };
    let acked = trigger.event();
    if !endpoints.contains(acked.entity) {
        // Packet was not sent by the endpoints used for RTT overhead estimation.
        return;
    }
    state.packet_rtts.push(acked.rtt_sample);
    if state.packet_rtts.len() < state.packet_count {
        return;
    }

    // Estimate the RTT overhead.
    state.packet_rtts.sort_unstable();
    let rtt_overhead = state.packet_rtts[state.packet_rtts.len() / 2];
    commands.trigger(RttOverheadEstimated(rtt_overhead));

    // The estimation is complete. We no longer need the state information nor the
    // endpoints.
    for endpoint in &endpoints {
        commands.entity(endpoint).despawn();
    }
    commands.remove_resource::<RttOverheadState>();
}

/// Sends one dummy payload per endpoint per frame, matching a real link's
/// once-per-frame packet flush so the ack wait is the same.
fn send_estimator_traffic(endpoints: Query<&Transport, With<EstimatorEndpoint>>) {
    for transport in &endpoints {
        transport
            .send::<EstimatorChannel>(Bytes::from_static(&[0]))
            .ok();
    }
}

/// Discards the dummy messages so the estimator's channel receivers don't
/// grow unboundedly.
fn drain_estimator_messages(mut endpoints: Query<&mut Transport, With<EstimatorEndpoint>>) {
    for mut transport in &mut endpoints {
        for metadata in transport.receivers.values_mut() {
            while metadata.receiver.read_message().is_some() {}
        }
    }
}

#[cfg(test)]
mod tests {
    use core::time::Duration;

    use lightyear_core::plugin::CorePlugins;

    use super::*;
    use crate::{channel::registry::ChannelRegistry, plugin::TransportPlugin};

    #[derive(Resource, Default)]
    struct CapturedOverhead(Option<Duration>);

    #[test]
    fn estimator_fires_once_the_window_fills() {
        let mut app = App::new();
        app.add_plugins(CorePlugins {
            tick_duration: Duration::from_millis(10),
        });
        if !app.is_plugin_added::<bevy_time::TimePlugin>() {
            app.add_plugins(bevy_time::TimePlugin);
        }
        app.init_resource::<ChannelRegistry>();
        app.add_plugins(TransportPlugin);
        app.add_plugins(RttOverheadEstimatorPlugin);
        app.init_resource::<CapturedOverhead>();
        app.add_observer(
            |trigger: On<RttOverheadEstimated>, mut captured: ResMut<CapturedOverhead>| {
                captured.0 = Some(trigger.event().0);
            },
        );
        app.finish();

        // Use a small packet count so the window fills within a few frames.
        app.world_mut()
            .trigger(EstimateRttOverhead { packet_count: 64 });

        // The estimate fires once the window fills. Both endpoints contribute
        // one sample per frame. Cap the loop so a regression fails instead of
        // hanging.
        for _ in 0..4096 {
            app.update();
            if app.world().resource::<CapturedOverhead>().0.is_some() {
                break;
            }
        }

        let overhead = app
            .world()
            .resource::<CapturedOverhead>()
            .0
            .expect("Estimator should fire once the window fills");

        // Verify that the RTT overhead estimate is a reasonable value.
        assert!(overhead < Duration::from_secs(1), "Overhead = {overhead:?}");

        // Verify that the RTT estimator tears itself down when it fires.
        assert!(
            !app.world().contains_resource::<RttOverheadState>(),
            "RTT overhead estimation state should be dropped once the estimate fires"
        );
    }
}
