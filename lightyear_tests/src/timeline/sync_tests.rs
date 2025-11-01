// use crate::protocol::*;
// use crate::stepper::*;
// use lightyear::prelude::*;
// use lightyear_sync::prelude::*;
// use lightyear_sync::prelude::client::*;
// use bevy::prelude::*;
// use std::time::Duration;
// use tracing::info;
//
// /// Test that the input timeline is synced to the remote timeline
// #[test_log::test]
// fn test_input_timeline_sync_normal() {
//     let mut stepper = ClientServerStepper::from_config(StepperConfig::single());
//
//     // Let a few frames pass to establish ping measurements
//     for _ in 0..10 {
//         stepper.frame_step(1);
//     }
//
//     // Get the timelines
//     let client = stepper.client(0);
//     let input_timeline = client.get::<InputTimeline>().unwrap();
//     let remote_timeline = client.get::<RemoteTimeline>().unwrap();
//
//     // Check that the input timeline's relative speed is close to 1.0 under normal conditions
//     assert!((input_timeline.relative_speed() - 1.0).abs() < 0.1,
//             "Expected input timeline speed to be close to 1.0, got {}", input_timeline.relative_speed());
//
//     // Check that the input timeline is ahead of the remote timeline by approximately RTT/2
//     let ping_manager = client.get::<PingManager>().unwrap();
//     let expected_ahead = Duration::from_nanos((ping_manager.rtt() / 2).as_nanos() as u64);
//     let actual_ahead = input_timeline.now().to_duration(input_timeline.tick_duration()) -
//                        remote_timeline.now().to_duration(remote_timeline.tick_duration());
//
//     info!("Expected ahead: {:?}, Actual ahead: {:?}", expected_ahead, actual_ahead);
//
//     // Allow some margin for error
//     assert!(actual_ahead.as_millis() as i64 - expected_ahead.as_millis() as i64 < 50,
//             "Input timeline should be ahead of remote timeline by approximately RTT/2");
// }
//
// /// Test that speed adjustments are made when timelines start to drift
// #[test_log::test]
// fn test_timeline_speed_adjustment() {
//     let mut stepper = ClientServerStepper::from_config(StepperConfig::single());
//
//     // Let a few frames pass to establish ping measurements
//     for _ in 0..10 {
//         stepper.frame_step(1);
//     }
//
//     // Artificially speed up the server timeline to create a drift
//     {
//         let server_app = &mut stepper.server;
//         let mut time = server_app.world_mut().resource_mut::<Time<Virtual>>();
//         time.set_relative_speed(1.2);
//     }
//
//     // Run several frames for the client to detect and adjust to the drift
//     for _ in 0..20 {
//         stepper.frame_step(1);
//     }
//
//     // The client should have adjusted its timeline speed to compensate
//     let client = stepper.client(0);
//     let input_timeline = client.get::<InputTimeline>().unwrap();
//
//     // The input timeline should be running faster than normal to catch up
//     assert!(input_timeline.relative_speed() > 1.0,
//             "Expected input timeline to speed up, speed is {}", input_timeline.relative_speed());
// }
//
// /// Test that a resync event is triggered when timelines are too far apart
// #[test_log::test]
// fn test_resync_event() {
//     let mut stepper = ClientServerStepper::from_config(StepperConfig::single());
//
//     // Let a few frames pass to establish ping measurements
//     for _ in 0..10 {
//         stepper.frame_step(1);
//     }
//
//     // Save the current input timeline state
//     let initial_time = {
//         let client = stepper.client(0);
//         let input_timeline = client.get::<InputTimeline>().unwrap();
//         input_timeline.now()
//     };
//
//     // Artificially create a large gap between server and client timelines
//     {
//         let server_app = &mut stepper.server;
//         let mut time = server_app.world_mut().resource_mut::<Time<Virtual>>();
//         time.set_relative_speed(5.0); // Major speed difference
//     }
//
//     // Run several frames for the resync to be triggered
//     for _ in 0..20 {
//         stepper.frame_step(1);
//     }
//
//     // Check that the input timeline has been resynced (it should now be at a significantly different time)
//     let current_time = {
//         let client = stepper.client(0);
//         let input_timeline = client.get::<InputTimeline>().unwrap();
//         input_timeline.now()
//     };
//
//     // The difference should be significant, indicating a resync occurred
//     let is_synced = stepper.client(0).contains::<IsSynced<InputTimeline>>();
//     assert!(is_synced, "Timeline should be marked as synced after resync");
//
//     info!("Initial time: {:?}, Current time: {:?}", initial_time, current_time);
//     // The timeline should have been adjusted
//     assert!(current_time != initial_time, "Timeline should have been resynced");
// }
//
// #[cfg(feature = "interpolation")]
// #[test_log::test]
// fn test_interpolation_timeline_sync() {
//     let mut stepper = ClientServerStepper::from_config(StepperConfig::single());
//
//     // Let a few frames pass to establish ping measurements
//     for _ in 0..10 {
//         stepper.frame_step(1);
//     }
//
//     let client = stepper.client(0);
//
//     // Check that both input and interpolation timelines are correctly initialized
//     let input_timeline = client.get::<InputTimeline>().unwrap();
//     let interpolation_timeline = client.get::<InterpolationTimeline>().unwrap();
//     let remote_timeline = client.get::<RemoteTimeline>().unwrap();
//
//     // The interpolation timeline should be behind the remote timeline
//     let remote_now = remote_timeline.now().to_duration(remote_timeline.tick_duration());
//     let interp_now = interpolation_timeline.now().to_duration(interpolation_timeline.tick_duration());
//
//     info!("Remote timeline: {:?}, Interpolation timeline: {:?}", remote_now, interp_now);
//     assert!(interp_now < remote_now,
//             "Interpolation timeline should be behind remote timeline");
//
//     // The input timeline should be ahead of the remote timeline
//     let input_now = input_timeline.now().to_duration(input_timeline.tick_duration());
//     assert!(input_now > remote_now,
//             "Input timeline should be ahead of remote timeline");
// }
//
// /// Test that the virtual time is updated based on the driving timeline
// #[test_log::test]
// fn test_virtual_time_update() {
//     let mut stepper = ClientServerStepper::from_config(StepperConfig::single());
//
//     // Let a few frames pass to establish ping measurements
//     for _ in 0..10 {
//         stepper.frame_step(1);
//     }
//
//     // Modify the input timeline's relative speed
//     {
//         let client = stepper.client_mut(0);
//         let mut input_timeline = client.get_mut::<InputTimeline>().unwrap();
//         input_timeline.set_relative_speed(1.5); // Set a different speed
//     }
//
//     // Run a frame to let the virtual time update
//     stepper.frame_step(1);
//
//     // Check that the virtual time's relative speed has been updated
//     let client = stepper.client(0);
//     let input_timeline = client.get::<InputTimeline>().unwrap();
//     let virtual_time = client.get::<Time<Virtual>>().unwrap();
//
//     assert_eq!(virtual_time.relative_speed(), input_timeline.relative_speed(),
//               "Virtual time should have the same relative speed as driving timeline");
// }
//
// /// Test that the input delay is correctly computed based on RTT
// #[test_log::test]
// fn test_input_delay_calculation() {
//     let mut stepper = ClientServerStepper::from_config(StepperConfig::single());
//
//     // Let a few frames pass to establish ping measurements
//     for _ in 0..10 {
//         stepper.frame_step(1);
//     }
//
//     // Get initial input delay
//     let initial_delay = {
//         let client = stepper.client(0);
//         let input_timeline = client.get::<InputTimeline>().unwrap();
//         input_timeline.context.input_delay_ticks
//     };
//
//     // Simulate increased network latency
//     {
//         let client = stepper.client_mut(0);
//         let mut ping_manager = client.get_mut::<PingManager>().unwrap();
//         // Triple the RTT
//         let new_rtt = ping_manager.rtt() * 3;
//         ping_manager.update_with_rtt(new_rtt);
//     }
//
//     // Force a sync event to trigger input delay recalculation
//     {
//         let client = stepper.client_mut(0);
//         let mut input_timeline = client.get_mut::<InputTimeline>().unwrap();
//         let remote_timeline = client.get::<RemoteTimeline>().unwrap().clone();
//         let ping_manager = client.get::<PingManager>().unwrap();
//
//         // Manually trigger a sync event
//         if let Some(sync_event) = input_timeline.sync(&remote_timeline, ping_manager) {
//             stepper.client_world_mut(0).send_event(sync_event);
//         }
//     }
//
//     // Run a frame to let the input delay update
//     stepper.frame_step(1);
//
//     // Check that the input delay has increased
//     let new_delay = {
//         let client = stepper.client(0);
//         let input_timeline = client.get::<InputTimeline>().unwrap();
//         input_timeline.context.input_delay_ticks
//     };
//
//     info!("Initial delay: {}, New delay: {}", initial_delay, new_delay);
//     assert!(new_delay > initial_delay, "Input delay should increase with higher RTT");
// }
