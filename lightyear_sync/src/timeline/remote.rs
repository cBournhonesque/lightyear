use crate::ping::manager::PingManager;
use crate::timeline::sync::SyncTargetTimeline;
use bevy::prelude::*;
use core::time::Duration;
use lightyear_connection::client::Connected;
use lightyear_core::prelude::Rollback;
use lightyear_core::tick::{Tick, TickDuration};
use lightyear_core::time::{TickDelta, TickInstant};
use lightyear_core::timeline::{NetworkTimeline, Timeline, TimelineContext};
use lightyear_link::Linked;
use lightyear_transport::plugin::PacketReceived;
use tracing::trace;

/// The local peer's estimate of the remote peer's timeline
///
/// This component maintains the local estimate of what time it is on a remote peer
/// based on received network packets and measured latency. It's primarily used to
/// synchronize game state between peers in a networked environment.
///
/// # Examples
///
/// ```
/// # use lightyear_sync::timeline::remote::RemoteTimeline;
/// # use lightyear_core::time::TickInstant;
/// # use std::time::Duration;
/// #
/// // Create a new remote estimate with a 16ms tick duration and 0.1 smoothing factor
/// let remote_estimate = RemoteTimeline::new(Duration::from_millis(16), 0.1);
/// ```
#[derive(Debug, Reflect)]
pub struct RemoteEstimate {
    /// Returns true if we have received a packet from the remote peer this frame
    received_packet: bool,
    /// Most recent tick received from the Server
    last_received_tick: Option<Tick>,
    /// Our estimate of the offset of the remote timeline compared with our estimate
    /// if we just updated the timeline without any adjustments
    ///
    /// We don't touch the remote estimate directly but only modify the offset.
    offset: TickDelta,
    /// Exponential smoothing factor for our estimate of the remote time
    /// Smaller values mean more smoothing but less responsiveness (works well in high-jitter situations)
    /// Bigger values means less smoothing and more responsiveness (works well in low-jitter situations)
    ///
    /// We will choose the actual smoothing factor based on the current jitter
    ///
    min_ema_alpha: f32,
    max_ema_alpha: f32,
    /// Number of handshake pings to be received before we start computing the offset
    handshake_pings: u32,
    /// Indicator for whether this is the first estimate or not
    first_estimate: bool,
}

impl Default for RemoteEstimate {
    fn default() -> Self {
        Self {
            received_packet: false,
            last_received_tick: None,
            offset: TickDelta::from(0),
            min_ema_alpha: 0.02,
            max_ema_alpha: 0.15,
            handshake_pings: 3,
            first_estimate: true,
        }
    }
}

// We need to wrap the inner Timeline to avoid the orphan rule
#[derive(Component, Default, Debug, Deref, DerefMut, Reflect)]
pub struct RemoteTimeline(Timeline<RemoteEstimate>);

impl TimelineContext for RemoteEstimate {}

impl RemoteTimeline {
    /// Returns the most recent tick received from the remote peer.
    ///
    /// # Returns
    ///
    /// An Option containing the most recent Tick if available, or None if no ticks have been received.
    ///
    /// # Examples
    ///
    /// ```
    /// # use lightyear_sync::timeline::remote::RemoteTimeline;
    /// # use std::time::Duration;
    /// #
    /// let remote_estimate = RemoteTimeline::default();
    /// assert_eq!(remote_estimate.last_received_tick(), None);
    /// ```
    pub fn last_received_tick(&self) -> Option<Tick> {
        self.context.last_received_tick
    }

    // TODO: maybe include remote overstep?
    /// Updates the local estimate after receiving a packet from the remote peer.
    ///
    /// This method uses the received tick and network latency information to
    /// update the estimate of the current time on the remote peer.
    ///
    /// # Arguments
    ///
    /// * `remote_tick` - The tick from the remote peer's timeline included in the received packet
    /// * `ping_manager` - Reference to the PingManager that tracks network latency measurements
    ///
    /// # Note
    ///
    /// This method will only update the estimate if the received tick is newer than
    /// the previously received tick.
    pub(crate) fn update(
        &mut self,
        remote_tick: Tick,
        ping_manager: &PingManager,
        tick_duration: Duration,
    ) {
        if ping_manager.pongs_recv < self.handshake_pings {
            return;
        }
        if self
            .context
            .last_received_tick
            .map_or(true, |previous_tick| remote_tick >= previous_tick)
        {
            // only update if the remote tick is newer than the last received tick
            self.context.received_packet = true;
            self.context.last_received_tick = Some(remote_tick);

            // we have received the packet now, so the remote must already be at RTT/2 ahead
            let network_delay = TickDelta::from_duration(ping_manager.rtt() / 2, tick_duration);
            let new_estimate = TickInstant::from(remote_tick) + network_delay;
            let ideal_estimate = self.now();

            let raw_offset = new_estimate - ideal_estimate;

            // for the first time, don't apply smoothing
            if self.context.first_estimate {
                self.offset = raw_offset;
                self.context.first_estimate = false;
            } else {
                // the smoothing depends on the amount of jitter
                let jitter_ms = ping_manager.jitter().as_millis() as f32;
                let alpha = self.ema_alpha(jitter_ms);
                let smoothed_offset = self.offset * (1.0 - alpha) + raw_offset * alpha;
                trace!(?new_estimate, ?ideal_estimate, old_offset = ?self.offset, new_offset = ?smoothed_offset, ?jitter_ms, ?alpha, "Update RemoteTimeline offset");
                self.offset = smoothed_offset;
            }
        }
    }

    /// On connection, reset the Synced timeline.
    pub(crate) fn handle_connect(
        trigger: Trigger<OnAdd, Connected>,
        mut query: Query<&mut RemoteTimeline>,
    ) {
        if let Ok(mut timeline) = query.get_mut(trigger.target()) {
            timeline.received_packet = false;
            timeline.offset = TickDelta::from(0);
            timeline.first_estimate = true;
            timeline.last_received_tick = None;
        }
    }

    /// Calculates a dynamic EMA smoothing factor (alpha) based on network jitter.
    ///
    /// The alpha value is determined as follows:
    /// - If jitter is 1ms or lower, alpha is 0.2.
    /// - If jitter is 5ms or higher, alpha is 0.03.
    /// - Between 1ms and 5ms jitter, alpha decreases linearly from 0.2 to 0.03.
    ///
    /// # Arguments
    ///
    /// * `current_jitter_ms`: The current measured jitter (standard deviation of RTT estimate)
    ///   in milliseconds.
    ///
    /// # Returns
    ///
    /// The calculated dynamic alpha value (f32).
    fn ema_alpha(&self, current_jitter_ms: f32) -> f32 {
        const JITTER_THRESHOLD_LOW_MS: f32 = 1.0;
        const JITTER_THRESHOLD_HIGH_MS: f32 = 5.0;

        let jitter_range = JITTER_THRESHOLD_HIGH_MS - JITTER_THRESHOLD_LOW_MS;
        let alpha_range = self.max_ema_alpha - self.min_ema_alpha;

        // 1. Calculate the normalized position of the jitter within the defined range.
        let normalized_jitter = (current_jitter_ms - JITTER_THRESHOLD_LOW_MS) / jitter_range;

        // 2. Clamp this factor to the range [0.0, 1.0].
        let clamped_normalized_jitter = normalized_jitter.clamp(0.0, 1.0);

        // 3. Linearly interpolate alpha based on clamped_normalized_jitter.
        let dynamic_alpha = self.max_ema_alpha - clamped_normalized_jitter * alpha_range;
        dynamic_alpha
    }
}

// TODO: instead of a trigger, should this be after MessageReceivedSet?
/// Update the timeline in FixedUpdate based on the Pings received
/// Should we use this only in FixedUpdate::First? because we need the tick in FixedUpdate to be correct for the timeline
pub(crate) fn update_remote_timeline(
    trigger: Trigger<PacketReceived>,
    tick_duration: Res<TickDuration>,
    mut query: Query<(&mut RemoteTimeline, &PingManager)>,
) {
    if let Ok((mut t, ping_manager)) = query.get_mut(trigger.target()) {
        trace!(
            "Received packet received with remote tick {:?}",
            trigger.remote_tick
        );
        t.update(trigger.remote_tick, ping_manager, tick_duration.0);
    }
}

// TODO: should this be based on real time?
/// Advance our estimate of the remote timeline based on the real time
pub(crate) fn advance_remote_timeline(
    fixed_time: Res<Time>,
    tick_duration: Res<TickDuration>,
    mut query: Query<&mut RemoteTimeline, (With<Linked>, Without<Rollback>)>,
) {
    let delta = fixed_time.delta();
    query.iter_mut().for_each(|mut t| {
        t.apply_duration(delta, tick_duration.0);
    })
}

/// Reset the bool that tracks if we received a packet this frame
pub(crate) fn reset_received_packet_remote_timeline(
    mut query: Query<&mut RemoteTimeline, With<Linked>>,
) {
    query.iter_mut().for_each(|mut t| {
        t.context.received_packet = false;
    });
}

impl SyncTargetTimeline for RemoteTimeline {
    fn current_estimate(&self) -> TickInstant {
        self.now + self.offset
    }

    fn received_packet(&self) -> bool {
        self.received_packet
    }
}
