/*! Handles syncing the time between the client and the server
*/
use bevy::prelude::{Reflect, SystemSet};
use chrono::Duration as ChronoDuration;
use core::time::Duration;
use tracing::{debug, trace};

use crate::client::interpolation::plugin::InterpolationConfig;
use crate::packet::packet::PacketId;
use crate::prelude::client::{InterpolationDelay, PredictionConfig};
use crate::shared::ping::manager::PingManager;
use crate::shared::tick_manager::TickManager;
use crate::shared::tick_manager::{Tick, TickEvent};
use crate::shared::time_manager::{TimeManager, WrappedTime};
use crate::utils::ready_buffer::ReadyBuffer;

/// SystemSet that holds systems that update the client's tick/time to match the server's tick/time
#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone, Copy)]
pub struct SyncSet;

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

#[derive(Default)]
pub struct SentPacketStore {
    buffer: ReadyBuffer<WrappedTime, PacketId>,
}

impl SentPacketStore {
    pub fn new() -> Self {
        Self {
            buffer: ReadyBuffer::new(),
        }
    }
}

/// In charge of syncing the client's tick/time with the server's tick/time
/// right after the connection is established
#[derive(Debug)]
pub struct SyncManager {
    config: SyncConfig,
    prediction_config: PredictionConfig,
    /// whether the handshake is finalized
    pub(crate) synced: bool,

    // time
    server_time_estimate: WrappedTime,
    pub(crate) interpolation_time: WrappedTime,
    interpolation_speed_ratio: f32,

    // ticks
    /// Number of input delay ticks to apply.
    ///
    /// This value is only updated when there are big changes in the RTT that warrant ping changes.
    /// (i.e. in the `finalize` function)
    ///
    /// It's not updated every frame otherwise the input delay would jitter constantly and some
    /// inputs would be overriden or ignored. Besides, we are already changing the client
    /// speed to stay in sync with the server, so we don't want to modify the input delay on top of that.
    pub(crate) current_input_delay: u16,
    // TODO: see if this is correct; should we instead attach the tick on every update message?
    /// Tick of the server that we last received in any packet from the server.
    /// This is not updated every tick, but only when we receive a packet from the server.
    pub(crate) latest_received_server_tick: Option<Tick>,
    pub(crate) duration_since_latest_received_server_tick: Duration,
    pub(crate) new_latest_received_server_tick: bool,
    /// The 'generation' of the tick. Everytime the tick wraps around, the generation increases by 1
    pub(crate) server_pong_generation: u16,
    /// The Tick associated with the 'server_tick_generation' (it might not be the same as latest_received_server_tick
    /// because we update the generation only from pong messages)
    pub(crate) server_pong_tick: Tick,
}

// TODO: split into PredictionTime Manager, InterpolationTime Manager
impl SyncManager {
    pub fn new(config: SyncConfig, prediction_config: PredictionConfig) -> Self {
        Self {
            config,
            prediction_config,
            synced: false,
            // time
            server_time_estimate: WrappedTime::default(),
            interpolation_time: WrappedTime::default(),
            interpolation_speed_ratio: 1.0,
            // server tick
            current_input_delay: 0,
            latest_received_server_tick: None,
            duration_since_latest_received_server_tick: Duration::default(),
            new_latest_received_server_tick: false,
            server_pong_generation: 0,
            server_pong_tick: Tick(0),
        }
    }

    /// We want to run this update at PostUpdate, after both ticks/time have been updated
    /// (because we need to compare the client tick with the server tick when the server sends packets,
    /// i.e. after both ticks/time have been updated)
    pub(crate) fn update(
        &mut self,
        time_manager: &mut TimeManager,
        tick_manager: &mut TickManager,
        ping_manager: &PingManager,
        prediction_config: &PredictionConfig,
        interpolation_delay: &InterpolationConfig,
        server_send_interval: Duration,
    ) -> Option<TickEvent> {
        // TODO: we are in PostUpdate, so this seems incorrect? this uses the previous-frame's delta,
        //  but instead we want to add the duration since the start of frame?
        self.duration_since_latest_received_server_tick += time_manager.delta();
        self.server_time_estimate += time_manager.delta();
        self.interpolation_time += time_manager.delta().mul_f32(self.interpolation_speed_ratio);

        // check if we are ready to finalize the handshake
        if !self.synced && ping_manager.sync_stats.len() >= self.config.handshake_pings as usize {
            self.synced = true;
            self.interpolation_time = self.interpolation_objective(
                interpolation_delay,
                server_send_interval,
                tick_manager,
            );
            debug!(
                interpolation_tick = ?self.interpolation_tick(tick_manager),
                "Client is synced!"
            );
            return self.finalize(time_manager, tick_manager, ping_manager, prediction_config);
        }

        if self.synced {
            self.update_interpolation_time(interpolation_delay, server_send_interval, tick_manager);
        }
        None
    }

    pub(crate) fn is_synced(&self) -> bool {
        self.synced
    }

    /// Compute the current client time from the client tick and the overstep.
    ///
    /// We use the client tick as the source of truth because the client tick can be
    /// reset via a TickEvent.
    ///
    /// we will make sure that the client tick is ahead of the server tick
    /// Even if it is wrapped around.
    /// (i.e. if client tick is 1, and server tick is 65535, we act as if the client tick was 65537)
    /// This is because we have 2 distinct entities with wrapping: Ticks and WrappedTime
    pub(crate) fn current_prediction_time(
        &self,
        tick_manager: &TickManager,
        time_manager: &TimeManager,
    ) -> WrappedTime {
        // NOTE: careful! We know that client tick should always be ahead of server tick.
        //  let's assume that this is the case after we did tick syncing
        //  so if we are behind, that means that the client tick wrapped around.
        //  for the purposes of the sync computations, the client tick should be ahead
        let client_tick_raw = tick_manager.tick().0 as i32;

        // client can only be this behind server if it wrapped around... if that's the case, we need to update
        // the generation to compute the time correctly
        // SAFETY: we only call this when we are synced, so we know that the latest_received_server_tick is not None
        // NOTE: we only call this when we are synced, so we know that the client tick is ahead of the server tick
        let generation = if (client_tick_raw - self.latest_received_server_tick.unwrap().0 as i32)
            < (i16::MIN as i32)
        {
            debug!("client tick is one generation ahead of server tick");
            self.server_latest_tick_generation() + 1
        } else {
            self.server_latest_tick_generation()
        };

        let res = WrappedTime::from_tick(
            tick_manager.tick(),
            generation,
            tick_manager.config.tick_duration,
        ) + tick_manager
            .config
            .tick_duration
            .mul_f32(time_manager.overstep());
        // when getting time from ticks, don't forget the overstep
        trace!(
            ?generation,
            current_tick = ?tick_manager.tick(),
            "current_prediction_time: {:?}", res);
        res
    }

    /// current server time from server's point of view (using server tick)
    pub(crate) fn server_time_estimate(&self) -> WrappedTime {
        self.server_time_estimate
    }

    fn server_latest_tick_generation(&self) -> u16 {
        // check if the latest_server_tick has crossed a generation compared to the latest pong tick
        if self.latest_received_server_tick.unwrap().0 < self.server_pong_tick.0 {
            debug!("latest server tick is a generation compared to the server pong tick");
            self.server_pong_generation + 1
        } else {
            self.server_pong_generation
        }
    }

    /// Everytime we receive a new server update:
    /// Update the estimated current server time, computed from the time elapsed since the
    /// latest received server tick, and our estimate of the RTT
    pub(crate) fn update_server_time_estimate(&mut self, tick_duration: Duration, rtt: Duration) {
        // TODO: should we add the time since
        // SAFETY: by that point we have received at least one server packet, so the latest_received_server_tick is not None
        let new_server_time_estimate = WrappedTime::from_tick(
            self.latest_received_server_tick.unwrap(),
            self.server_latest_tick_generation(),
            tick_duration,
        ) + self.duration_since_latest_received_server_tick;

        // instead of just using the latest_received_server_tick, we apply some smoothing
        // (in case the latest server tick is wildly off-base)
        // TODO: should we do this only after syncing? because otherwise the server_estimate
        //  might be earlier than what we compute using the server tick
        if self.server_time_estimate == WrappedTime::default() || !self.is_synced() {
            self.server_time_estimate = new_server_time_estimate;
        } else {
            self.server_time_estimate = self.server_time_estimate
                * self.config.server_time_estimate_smoothing
                + new_server_time_estimate * (1.0 - self.config.server_time_estimate_smoothing);
        }
        trace!(
            ?new_server_time_estimate,
            updated_server_time_estimate = ?self.server_time_estimate,
            ?self.latest_received_server_tick,
            ?self.duration_since_latest_received_server_tick,
            ?rtt,
            "updated server time estimate"
        );
    }

    /// time (from server's scale) at which the server would receive a packet we send now
    fn predicted_server_receive_time(&self, rtt: Duration) -> WrappedTime {
        self.server_time_estimate() + rtt
    }

    /// How far ahead of the server should I be? (for prediction)
    ///
    /// We want the input packets for tick T sent from the client to arrive on the server at tick T.
    /// So the client should be ahead by RTT/2 - input_delay_ticks
    ///
    /// This could be a negative value
    fn client_ahead_minimum(&self, tick_duration: Duration, jitter: Duration) -> ChronoDuration {
        // TODO: do we need to make sure that the client time is ahead of the server time?
        //  we might have some weird interpolation issues if this is not the case
        let input_delay = tick_duration * self.current_input_delay as u32;
        ChronoDuration::nanoseconds(
            jitter.as_nanos() as i64 * self.config.jitter_multiple_margin as i64
                // TODO: this should actually be `n * client_input_send_interval`
                //  in our case we send input messages in FixedUpdate, so roughly every tick_duration
                //  so this should be fine
                + tick_duration.as_nanos() as i64 * self.config.tick_margin as i64
                - input_delay.as_nanos() as i64,
        )
    }

    /// Returns what we think the client time should be, given the current server time estimate
    /// and the jitter/input_delay
    fn client_ideal_time(
        &self,
        rtt: Duration,
        tick_duration: Duration,
        jitter: Duration,
    ) -> WrappedTime {
        let ideal_time = self.predicted_server_receive_time(rtt)
            + self.client_ahead_minimum(tick_duration, jitter);

        // TODO: client_ideal_time must be higher than server_time in raw value (not wrapping)
        //  so that wrapping with Ticks still works!! Need to update this

        // TODO: is there a problem if the client_time is lower than server time? maybe not actually!

        // if the ideal time is too close to the server time (probably because of input delay)
        // make sure that the client time is still ahead of the server time
        core::cmp::max(
            ideal_time,
            // TODO: create setting for this. Maybe use one Tick duration?
            self.server_time_estimate() + tick_duration,
        )
    }

    pub(crate) fn interpolation_objective(
        &self,
        // TODO: make interpolation delay part of SyncConfig?
        interpolation_delay: &InterpolationConfig,
        // TODO: should we get this via an estimate?
        server_send_interval: Duration,
        tick_manager: &TickManager,
    ) -> WrappedTime {
        // // TODO: maybe integrate because of jitter?
        // let objective_time = WrappedTime::from_duration(
        //     self.latest_received_server_tick.0 as u32 * tick_manager.config.tick_duration
        //         + self.duration_since_latest_received_server_tick,
        // );
        // let objective_time = self.server_time_estimate();
        // how much we want interpolation time to be behind the latest received server tick?
        // TODO: use a specified config margin + add std of time_between_server_updates?
        let objective_delta =
            chrono::Duration::from_std(interpolation_delay.to_duration(server_send_interval))
                .unwrap();
        // info!("objective_delta: {:?}", objective_delta);
        self.server_time_estimate() - objective_delta
    }

    /// Interpolation delay as the number of milliseconds between the prediction time and the interpolation time
    pub(crate) fn interpolation_delay(
        &self,
        tick_manager: &TickManager,
        time_manager: &TimeManager,
    ) -> InterpolationDelay {
        let prediction_time = self.current_prediction_time(tick_manager, time_manager);
        let delta = prediction_time - self.interpolation_time;
        assert!(
            delta.num_milliseconds() >= 0,
            "the prediction time should always be ahead of the interpolation time!"
        );
        InterpolationDelay {
            delay_ms: delta.num_milliseconds() as u16,
        }
    }

    pub(crate) fn interpolation_tick(&self, tick_manager: &TickManager) -> Tick {
        self.interpolation_time
            .to_tick(tick_manager.config.tick_duration)
    }

    pub(crate) fn interpolation_overstep(&self, tick_manager: &TickManager) -> f32 {
        self.interpolation_time
            .tick_overstep(tick_manager.config.tick_duration)
    }

    // TODO: only run when there's a change? (new server tick received or new ping received)
    // TODO: change name to make it clear that we might modify speed
    pub(crate) fn update_interpolation_time(
        &mut self,
        // TODO: make interpolation delay part of SyncConfig?
        interpolation_delay: &InterpolationConfig,
        // TODO: should we get this via an estimate?
        server_update_rate: Duration,
        tick_manager: &TickManager,
    ) {
        // for interpolation time, we don't need to use ticks (because we only need interpolation at the end
        // of the frame, not during the FixedUpdate schedule)
        let objective_time =
            self.interpolation_objective(interpolation_delay, server_update_rate, tick_manager);
        let delta = objective_time - self.interpolation_time;
        trace!(
            ?objective_time,
            interpolation_time = ?self.interpolation_time,
            interpolation_tick = ?self.interpolation_tick(tick_manager),
            "interpolation data");

        let max_error_margin_time = chrono::Duration::from_std(
            tick_manager
                .config
                .tick_duration
                .mul_f32(self.config.max_error_margin),
        )
        .unwrap();
        if delta > max_error_margin_time || delta < -max_error_margin_time {
            debug!(
                ?objective_time,
                interpolation_time = ?self.interpolation_time,
                "Error too big, snapping interpolation time/tick to objective",
            );
            self.interpolation_time = objective_time;
            return;
        }

        // TODO: make this configurable
        let error_margin = chrono::Duration::milliseconds(10);
        if delta > error_margin {
            // interpolation time is too far behind, speed-up!
            self.interpolation_speed_ratio = 1.0 * self.config.speedup_factor;
            trace!("interpolation is too far behind, speed up!");
        } else if delta < -error_margin {
            trace!("interpolation is too far ahead, slow down!");
            self.interpolation_speed_ratio = 1.0 / self.config.speedup_factor;
        } else {
            self.interpolation_speed_ratio = 1.0;
        }
    }

    /// Update the client time ("upstream-throttle"): speed-up or down depending on the
    /// The objective of update-client-time is to make sure the client packets for tick T arrive on server before server reaches tick T
    /// but not too far ahead
    pub(crate) fn update_prediction_time(
        &mut self,
        time_manager: &mut TimeManager,
        tick_manager: &mut TickManager,
        ping_manager: &PingManager,
        prediction_config: &PredictionConfig,
    ) -> Option<TickEvent> {
        let rtt = ping_manager.rtt();
        let jitter = ping_manager.jitter();
        // current client time
        let current_prediction_time = self.current_prediction_time(tick_manager, time_manager);

        // client ideal time
        let client_ideal_time =
            self.client_ideal_time(rtt, tick_manager.config.tick_duration, jitter);

        let error = current_prediction_time - client_ideal_time;
        let error_margin_time = chrono::Duration::from_std(
            tick_manager
                .config
                .tick_duration
                .mul_f32(self.config.error_margin),
        )
        .unwrap();
        let max_error_margin_time = chrono::Duration::from_std(
            tick_manager
                .config
                .tick_duration
                .mul_f32(self.config.max_error_margin),
        )
        .unwrap();

        #[cfg(feature = "metrics")]
        {
            metrics::gauge!("sync::prediction_time::error_ms").set(error.num_milliseconds() as f64);
        }
        if error > max_error_margin_time || error < -max_error_margin_time {
            debug!(
                ?rtt,
                ?jitter,
                ?current_prediction_time,
                ?client_ideal_time,
                // stats = ?ping_manager.sync_stats,
                latest_received_server_tick = ?self.latest_received_server_tick,
                client_tick = ?tick_manager.tick(),
                error_ms = ?error.num_milliseconds(),
                error_margin_time_ms = ?error_margin_time.num_milliseconds(),
                "Error too big, snapping prediction time/tick to objective",
            );

            return self.finalize(time_manager, tick_manager, ping_manager, prediction_config);
        }

        time_manager.sync_relative_speed = if error > error_margin_time {
            debug!(
                ?rtt,
                ?jitter,
                ?current_prediction_time,
                ?client_ideal_time,
                latest_received_server_tick = ?self.latest_received_server_tick,
                client_tick = ?tick_manager.tick(),
                error_ms = ?error.num_milliseconds(),
                error_margin_time_ms = ?error_margin_time.num_milliseconds(),
                "Too far ahead of server! Slow down!",
            );
            // we are too far ahead of the server, slow down
            1.0 / self.config.speedup_factor
        } else if error < -error_margin_time {
            debug!(
                ?rtt,
                ?jitter,
                ?current_prediction_time,
                ?client_ideal_time,
                latest_received_server_tick = ?self.latest_received_server_tick,
                client_tick = ?tick_manager.tick(),
                error_ms = ?error.num_milliseconds(),
                error_margin_time_ms = ?error_margin_time.num_milliseconds(),
                "Too far behind of server! Speed up!",
            );
            // we are too far behind the server, speed up
            1.0 * self.config.speedup_factor
        } else {
            // we are within margins
            trace!("good speed");
            1.0
        };
        None
    }

    // Update internal time using offset so that times are synced.
    // This happens when a necessary # of handshake pongs have been recorded
    // Compute the final RTT/offset and set the client tick accordingly
    pub fn finalize(
        &mut self,
        time_manager: &mut TimeManager,
        tick_manager: &mut TickManager,
        ping_manager: &PingManager,
        prediction_config: &PredictionConfig,
    ) -> Option<TickEvent> {
        let tick_duration = tick_manager.config.tick_duration;
        let rtt = ping_manager.rtt();
        let jitter = ping_manager.jitter();
        // recompute the input delay using the new rtt estimate
        self.current_input_delay = prediction_config.input_delay_ticks(rtt, tick_duration);
        #[cfg(feature = "metrics")]
        {
            metrics::gauge!("inputs::input_delay_ticks").set(self.current_input_delay as f64);
        }
        // recompute the server time estimate (using the rtt we just computed)
        self.update_server_time_estimate(tick_duration, rtt);

        // Compute how many ticks the client must be compared to server
        let client_ideal_time = self.client_ideal_time(rtt, tick_duration, jitter);

        // TODO: client_ideal_time must be higher than server_time in raw value (not wrapping)
        //  so that wrapping with Ticks still works
        // if client_ideal_time < self.predicted_server_receive_time(rtt) {
        //     client_ideal_time
        // }

        // TODO: should we add 1 to get the div_ceil?
        // TODO: check that this wraps correctly?
        let client_ideal_tick =
            Tick((client_ideal_time.elapsed.as_nanos() / tick_duration.as_nanos()) as u16);

        let delta_tick = client_ideal_tick - tick_manager.tick();
        // Update client ticks
        if rtt != Duration::default() {
            debug!(
                buffer_len = ?ping_manager.sync_stats.len(),
                ?rtt,
                ?jitter,
                ?delta_tick,
                ?self.current_input_delay,
                predicted_server_receive_time = ?self.predicted_server_receive_time(rtt),
                client_ahead_time = ?self.client_ahead_minimum(tick_duration, jitter),
                ?client_ideal_time,
                ?client_ideal_tick,
                server_tick = ?self.latest_received_server_tick,
                client_current_tick = ?tick_manager.tick(),
                "Finished syncing!"
            );
        }
        Some(tick_manager.set_tick_to(client_ideal_tick))
    }
}

#[cfg(test)]
mod tests {
    use core::time::Duration;

    use crate::prelude::server::Replicate;
    use crate::prelude::*;
    use crate::tests::protocol::*;
    use crate::tests::stepper::BevyStepper;

    use super::*;

    /// Check that after a big tick discrepancy between server/client, the client tick gets updated
    /// to match the server tick
    #[test]
    fn test_sync_after_tick_wrap() {
        let tick_duration = Duration::from_millis(10);
        let mut stepper = BevyStepper::default();

        // set time to end of wrapping
        let new_tick = Tick(u16::MAX - 1000);
        let new_time = WrappedTime::from_duration(tick_duration * (new_tick.0 as u32));

        stepper
            .server_app
            .world_mut()
            .resource_mut::<TimeManager>()
            .set_current_time(new_time);
        stepper
            .server_app
            .world_mut()
            .resource_mut::<TickManager>()
            .set_tick_to(new_tick);

        let server_entity = stepper
            .server_app
            .world_mut()
            .spawn((ComponentSyncModeFull(0.0), Replicate::default()))
            .id();

        // cross tick boundary
        for i in 0..200 {
            stepper.frame_step();
        }
        stepper
            .server_app
            .world_mut()
            .entity_mut(server_entity)
            .insert(ComponentSyncModeFull(1.0));
        // dbg!(&stepper.server_tick());
        // dbg!(&stepper.client_tick());
        // dbg!(&stepper
        //     .server_app
        //     .world()
        //     .get::<ComponentSyncModeFull>(server_entity));

        // make sure the client receives the replication message
        for i in 0..5 {
            stepper.frame_step();
        }

        let client_entity = stepper
            .client_app
            .world()
            .resource::<client::ConnectionManager>()
            .replication_receiver
            .remote_entity_map
            .get_local(server_entity)
            .unwrap();
        assert_eq!(
            stepper
                .client_app
                .world()
                .get::<ComponentSyncModeFull>(client_entity)
                .unwrap(),
            &ComponentSyncModeFull(1.0)
        );
    }
}
