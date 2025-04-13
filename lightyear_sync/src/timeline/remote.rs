use crate::ping::manager::PingManager;
use bevy::prelude::{Component, Deref, DerefMut, Query, Real, Reflect, Res, Time, Trigger};
use core::time::Duration;
use lightyear_core::tick::Tick;
use lightyear_core::time::{TickDelta, TickInstant, TimeDelta};
use lightyear_core::timeline::{NetworkTimeline, Timeline};
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
#[derive(Default, Debug, Reflect)]
pub struct RemoteEstimate {
    /// Most recent tick received from the Server
    last_received_tick: Option<Tick>,
    /// Exponential smoothing factor for our estimate of the remote time
    /// Values closer to 0 give higher weight to new measurements,
    /// values closer to 1 give higher weight to the existing estimate.
    remote_estimate_smoothing: f32,

    /// Indicator for whether this is the first estimate or not
    first_estimate: bool,
}


// We need to wrap the inner Timeline to avoid the orphan rule
#[derive(Component, Default, Debug, Deref, DerefMut)]
pub struct RemoteTimeline(Timeline<RemoteEstimate>);

impl RemoteTimeline  {
    /// Returns the most recent tick received from the remote peer.
    ///
    /// # Returns
    ///
    /// An Option containing the most recent Tick if available, or None if no ticks have been received.
    ///
    /// # Examples
    ///
    /// ```
    /// # use lightyear_sync::timeline::remote::RemoteEstimateTimeline;
    /// # use std::time::Duration;
    /// #
    /// let remote_estimate = RemoteEstimateTimeline::new(Duration::from_millis(16), 0.2);
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
    pub(crate) fn update(&mut self, remote_tick: Tick, ping_manager: &PingManager) {
        if self.context.last_received_tick
           .map_or(true, |previous_tick| remote_tick >= previous_tick) {
            self.context.last_received_tick = Some(remote_tick);

            // TODO: should we make any adjustments?

            // we have received the packet now, so the remote must already be at RTT/2 ahead
            let network_delay = TickDelta::from_duration(ping_manager.rtt() / 2, self.tick_duration());
            let new_estimate = TickInstant::from(remote_tick) + network_delay;

            // for the first time, don't apply smoothing
            if self.context.first_estimate {
                self.now = new_estimate;
            } else {
                // we transform the instant into deltas to apply some transformations.
                // not all transformations are safe, but these are
                let smoothed_estimate = TickDelta::from(self.now) * self.context.remote_estimate_smoothing + TickDelta::from(new_estimate) * (1.0 - self.context.remote_estimate_smoothing);
                self.now = smoothed_estimate.into();
            }
            trace!(
                update_estimate = ?new_estimate,
                new_estimate = ?self.now,
                "updated remote timeline estimate"
            );
        }
    }

}

// TODO: instead of a trigger, should this be after MessageReceivedSet?
/// Update the timeline in FixedUpdate based on the Pings received
/// Should we use this only in FixedUpdate::First? because we need the tick in FixedUpdate to be correct for the timeline
pub(crate) fn update_remote_timeline(
    trigger: Trigger<PacketReceived>,
    mut query: Query<(&mut RemoteTimeline, &PingManager)>,
) {
    if let Ok((mut t, ping_manager)) = query.get_mut(trigger.target()) {
        t.update(trigger.remote_tick, ping_manager);
    }
}

/// Advance our estimate of the remote timeline based on the real time
pub(crate) fn advance_remote_timeline(
    fixed_time: Res<Time<Real>>,
    mut query: Query<&mut RemoteTimeline>,
) {
    let delta = fixed_time.delta();
    query.iter_mut().for_each(|mut t| {
        t.advance(delta);
    })
}
