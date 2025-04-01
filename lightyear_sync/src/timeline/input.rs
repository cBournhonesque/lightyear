use crate::ping::manager::PingManager;
use crate::timeline::sync::{SyncConfig, SyncEvent, SyncedTimeline};
use crate::timeline::{NetworkTimeline, Timeline};
use bevy::prelude::{Component, Reflect};
use core::time::Duration;
use lightyear_core::tick::Tick;
use lightyear_core::time::{TickDelta, TickInstant};

// TODO: maybe rename this as MainTimeline? It's independent from Prediction!
//   and in prediction we can just use the Main timeline as our timeline

/// Timeline that is used to make sure that Inputs from this peer will arrive on time
/// on the remote peer
#[derive(Default, Debug, Clone, Copy, Reflect)]
pub struct Input {
    pub(crate) config: SyncConfig,
    /// Current input_delay_ticks that are being applied
    pub(crate) input_delay_ticks: u16,
    relative_speed: f32,
}



impl SyncedTimeline for Timeline<Input> {
    // TODO: how can we make this configurable? or maybe just store the TICK_DURATION in the timeline itself?


    /// We want the Predicted timeline to be:
    /// - RTT/2 ahead of the server timeline, so that inputs sent from the server arrive on time
    /// - On top of that, we will take a bit of margin based on the jitter
    /// - we can reduce the ahead-delay by the input_delay
    /// Because of the input-delay, the time we return might be in the past compared with the main timeline
    fn sync_objective<T: NetworkTimeline>(&self, main: &T, ping_manager: &PingManager) -> TickInstant {
        // TODO: should we do current estimate? or Server::now() already does that?
        let target = main.now();
        let network_delay = TickDelta::from_duration(ping_manager.rtt() / 2, self.tick_duration());
        let jitter_margin = TickDelta::from_duration(ping_manager.jitter() * self.context.config.jitter_multiple_margin as u32 + self.tick_duration() * self.context.config.tick_margin as u32, self.tick_duration());
        let input_delay: TickDelta = Tick(self.context.input_delay_ticks).into();
        target + network_delay + jitter_margin - input_delay
    }

    fn resync(&mut self, sync_objective: TickInstant) -> SyncEvent<Self> {
        let now = self.now();
        let target = sync_objective;
        self.now = target;
        SyncEvent {
            old: now,
            new: target,
            marker: core::marker::PhantomData
        }
    }

    /// Adjust the current timeline to stay in sync with the [`MainTimeline`].
    ///
    /// Most of the times this will just be slight nudges to modify the speed of the [`SyncedTimeline`].
    /// If there's a big discrepancy, we will snap the [`SyncedTimeline`] to the [`MainTimeline`] by sending a SyncEvent
    fn sync<T: NetworkTimeline>(&mut self, main: &T, ping_manager: &PingManager) -> Option<SyncEvent<Self>> {
        // skip syncing if we haven't received enough information
        if ping_manager.pongs_recv < self.context.config.handshake_pings as u32 {
            return None
        }
        // TODO: should we call current_estimate()? now() should basically return the same thing
        let target = main.now();
        let objective = self.sync_objective(main, ping_manager);

        let error = objective - target;
        let is_ahead = error.is_positive();
        let error_duration = error.to_duration(self.tick_duration());
        let error_margin = self.tick_duration().mul_f32(self.context.config.error_margin);
        let max_error_margin = self.tick_duration().mul_f32(self.context.config.max_error_margin);
        if error_duration > max_error_margin {
            return Some(self.resync(objective));
        } else if error_duration > error_margin {
            let ratio = if is_ahead {
                1.0 / self.context.config.speedup_factor
            } else {
                1.0 * self.context.config.speedup_factor
            };
            self.set_relative_speed(ratio);
        }
        None
    }


    // TODO: do we want this or do we want a marker component to check if the timline is synced?
    fn is_synced(&self) -> bool {
        todo!()
    }

    fn relative_speed(&self) -> f32 {
        self.context.relative_speed
    }

    fn set_relative_speed(&mut self, ratio: f32) {
        self.context.relative_speed = ratio;
    }
}

// impl Predicted {
//
//     /// We want the Predicted timeline to be:
//     /// - RTT/2 ahead of the server timeline, so that inputs sent from the server arrive on time
//     /// - On top of that, we will take a bit of margin based on the jitter
//     /// - we can reduce the ahead-delay by the input_delay
//     /// Because of the input-delay, the time we return might be in the past compared with the main timeline
//     fn sync_objective<T: MainTimeline>(&self, main: &T, ping_manager: &PingManager) -> TickInstant {
//         // TODO: should we do current estimate? or Server::now() already does that?
//         let target = main.now();
//         let network_delay = TickDuration::from_duration(ping_manager.rtt() / 2, self.tick_duration());
//         let jitter_margin = TickDuration::from_duration(ping_manager.jitter() * self.config.jitter_multiple_margin as u32 + self.tick_duration() * self.config.tick_margin as u32, self.tick_duration());
//         let input_delay: TickDuration =  Tick(self.input_delay_ticks).into();
//         target + network_delay + jitter_margin - input_delay
//     }
//
//     fn resync(&mut self, sync_objective: TickInstant) -> SyncEvent<Self> {
//         let now = self.now();
//         let target = sync_objective;
//         self.now = target;
//         SyncEvent {
//             old: now,
//             new: target,
//         }
//     }
// }