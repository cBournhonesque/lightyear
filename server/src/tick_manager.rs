// use lightyear_shared::tick::Tick;
// use lightyear_shared::{TickConfig, TimeManager, WrappedTime};
// use tracing::{info, trace};
//
// pub struct TickManager {
//     config: TickConfig,
//     /// current tick (counter of fixed-update schedule runs)
//     tick: Tick,
//     /// time when we switched to the current tick
//     last_tick_wrapped_time: WrappedTime,
// }
//
// // TODO: THIS IS SERVER TIME MANAGER.
// impl TickManager {
//     pub fn from_config(config: TickConfig) -> Self {
//         Self {
//             config,
//             tick: Tick(0),
//             last_tick_wrapped_time: WrappedTime::new(0),
//         }
//     }
//
//     pub fn increment_tick(&mut self) {
//         self.tick += 1;
//         trace!(new_tick = ?self.tick, "incremented server tick")
//     }
//
//     // /// Update the status of the TickManager after time advances by `elapsed`
//     // /// Returns true if we changed ticks
//     // pub fn update(&mut self, time_manager: &TimeManager) -> bool {
//     //     let time_offset = time_manager.current_time() - self.last_tick_wrapped_time;
//     //     if time_offset > self.config.tick_duration {
//     //         // TODO: compute the actual tick duration
//     //         self.tick += 1;
//     //         self.last_tick_wrapped_time = time_manager.current_time();
//     //         return true;
//     //     }
//     //     return false;
//     // }
//
//     pub fn current_tick(&self) -> Tick {
//         self.tick
//     }
//
//     /// Receive a client ping containing: client tick, client timestamp
//     /// Send back:
//     /// - server timestamp when receiving client message, server tick,
//     pub fn process_client_ping(&mut self) {}
// }
