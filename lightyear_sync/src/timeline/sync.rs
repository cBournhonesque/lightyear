use crate::ping::manager::PingManager;
use crate::timeline::Timeline;
use bevy::prelude::{Event, Reflect};
use lightyear_core::time::{TickDuration, TickInstant};

#[derive(Event, Debug, Clone, Copy)]
pub struct SyncEvent<T: SyncedTimeline> {
    pub(crate) old: TickInstant,
    pub(crate) new: TickInstant,
}

/// Timeline that is synced to another timeline
pub trait SyncedTimeline: Timeline {

    /// Get the ideal [`TickInstant`] that this timeline should be at
    fn sync_objective<T: Timeline>(&self, other: &T, ping_manager: &PingManager) -> TickDuration;

    fn resync(&mut self, sync_objective: TickInstant) -> SyncEvent<Self>;

    /// Sync the current timeline to the other timeline T.
    /// Usually this is achieved by slightly speeding up or slowing down the current timeline.
    /// If there is a big discrepancy we can do a `resync` instead.
    fn sync<T: Timeline>(&mut self, main: &T, ping_manager: &PingManager) -> Option<SyncEvent<Self>>;

    fn is_synced(&self) -> bool;

    /// Returns the speed of your timeline relative to your system clock as an `f32`.
    /// A value of `1.0` means the timeline is running at normal speed.
    /// A value of `0.5` means the timeline is running at half speed,
    fn relative_speed(&self) -> f32;

    fn set_relative_speed(&mut self, ratio: f32);

}


/// Configuration for the sync manager, which is in charge of syncing the client's tick/time with the server's tick/time
///
/// The sync manager runs only on the client and maintains two different times:
/// - the prediction tick/time: this is the client time, which runs roughly RTT/2 ahead of the server time, so that input packets
///     for tick T sent from the client arrive on the server at tick T
/// - the interpolation tick/time: this is the interpolation timeline, which runs behind the server time so that interpolation
///     always has at least one packet to interpolate towards
#[derive(Clone, Copy, Debug, Reflect)]
pub struct SyncConfig {
    /// How much multiple of jitter do we apply as margin when computing the time
    /// a packet will get received by the server
    /// (worst case will be RTT / 2 + jitter * multiple_margin)
    /// % of packets that will be received within k * jitter
    /// 1: 65%, 2: 95%, 3: 99.7%
    pub jitter_multiple_margin: u8,
    /// How many ticks to we apply as margin when computing the time
    ///  a packet will get received by the server
    pub tick_margin: u8,
    /// Number of pings to exchange with the server before finalizing the handshake
    pub handshake_pings: u8,
    /// Error margin for upstream throttle (in multiple of ticks)
    pub error_margin: f32,
    /// If the error margin is too big, we snap the prediction/interpolation time to the objective value
    pub max_error_margin: f32,
    // TODO: instead of constant speedup_factor, the speedup should be linear w.r.t the offset
    /// By how much should we speed up the simulation to make ticks stay in sync with server?
    pub speedup_factor: f32,

    // Integration
    pub server_time_estimate_smoothing: f32,
}

impl Default for SyncConfig {
    fn default() -> Self {
        SyncConfig {
            jitter_multiple_margin: 3,
            tick_margin: 1,
            handshake_pings: 3,
            error_margin: 0.5,
            max_error_margin: 5.0,
            speedup_factor: 1.05,
            // server_time_estimate_smoothing: 0.0,
            server_time_estimate_smoothing: 0.2,
        }
    }
}

impl SyncConfig {
    pub fn speedup_factor(mut self, speedup_factor: f32) -> Self {
        self.speedup_factor = speedup_factor;
        self
    }
}