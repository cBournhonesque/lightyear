/*! Handles syncing the time between the client and the server
*/
use crate::plugin::{SyncPlugin, SyncedTimelinePlugin};
use crate::prelude::client::RemoteTimeline;
use crate::prelude::InputTimeline;
use crate::timeline::input::Input;
#[cfg(feature = "interpolation")]
use crate::timeline::interpolation::InterpolationTimeline;
use crate::timeline::remote;
use crate::timeline::sync::{SyncEvent, SyncedTimeline};
use bevy::prelude::*;
use lightyear_connection::client::Client;
use lightyear_core::prelude::{LocalTimeline, NetworkTimeline, NetworkTimelinePlugin};
use lightyear_core::time::TickDelta;

// When a Client is created; we want to add a PredictedTimeline? InterpolatedTimeline?
//  or should we let the user do it?
// Systems we need:
//  - We want FixedUpdate to slow down if Predicted timeline slows down, because FixedUpdate is fundamentally
//      what decides
//  - we update
pub struct ClientPlugin;

impl ClientPlugin {
    pub fn update_local_timeline(
        trigger: Trigger<SyncEvent<InputTimeline>>,
        mut query: Query<&mut LocalTimeline>,
    ) {
        if let Ok(mut timeline) = query.get_mut(trigger.target()) {
            timeline.apply_delta(TickDelta::from_i16(trigger.tick_delta));
        }
    }
}

// TODO: we might need a separate Predicted<Virtual> and Predicted<FixedUpdate>, and Predicted<()> fetches the correct one
//  depending on the Schedule? exactly like bevy does
//  and so that the Time is updated based on whether we're in Update

// First
//  - Time<Virtual>/Time<()> advance by delta
//  - Advance Predicted<()> and Predicted<Virtual> by delta * 1.0 (the predicted timeline is the main timeline so we purely match)
//  - Advance Interpolated<()> and Interpolated<Virtual> by delta
// FixedUpdate:
//  - Advance Predicted<Fixed> and Interpolated<Fixed> by accumulation
// PostUpdate:
//  - Sync timelines in PostUpdate because the server sends messages in PostUpdate (however maybe that's not relevant
//    because the server time is updated in First? Think about it) But we receive the server's Tick at frame end
//    (after the server ran FixedUpdateLoop)
//  - Update the Predicted<Virtual> and Interpolated<Virtual> relative speeds
//  - Set the relative speed of Time<Virtual> to Predicted<Virtual>'s relative speed

// Let's handle the Context later! it's a bit tricky
// Maybe this is confusing? What if we tried updating the timeline only in FixedUpdate?
//    - in FixedUpdate the tick/overstep would be correct
//    - in PostUpdate too
//    - in PreUpdate the Time<Virtual> has been updated but not the timelines! Maybe we could just store a PreUpdate now()?




impl Plugin for ClientPlugin {

    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<SyncPlugin>() {
            app.add_plugins(SyncPlugin);
        }

        app.register_required_components::<Client, InputTimeline>();
        app.register_required_components::<Client, RemoteTimeline>();


        app.add_observer(Input::recompute_input_delay);

        // TODO: should the DrivingTimeline be configurable?
        // the client will use the Input timeline as the driving timeline
        app.add_plugins(SyncedTimelinePlugin::<InputTimeline, RemoteTimeline, true>::default());

        #[cfg(feature = "interpolation")]
        {
            app.register_required_components::<Client, InterpolationTimeline>();
            app.add_plugins(SyncedTimelinePlugin::<InterpolationTimeline, RemoteTimeline>::default());
        }

        // remote timeline
        app.add_plugins(NetworkTimelinePlugin::<RemoteTimeline>::default());
        app.add_observer(remote::update_remote_timeline);
        // app.add_systems(First, remote::advance_remote_timeline.after(time_system));
        app.add_systems(FixedFirst, remote::advance_remote_timeline);
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use bevy::app::App;
    use bevy::time::{TimePlugin, TimeUpdateStrategy};
    use lightyear_core::prelude::Tick;
    use lightyear_core::tick::TickDuration;
    use lightyear_core::time::{Overstep, TickInstant};
    use std::time::{Duration, Instant};
    use test_log::test;

    #[test]
    fn test_advance_remote() {
        let mut app = App::new();
        let now = Instant::now();
        app.world_mut().insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(10)));
        app.world_mut().insert_resource(TickDuration(Duration::from_millis(10)));
        app.add_plugins((
            TimePlugin,
            ClientPlugin
        ));
        app.update();

        let e = app.world_mut().spawn(RemoteTimeline::default()).id();
        assert_eq!(app.world().get::<RemoteTimeline>(e).unwrap().now, TickInstant {
            tick: Tick(0),
            overstep: Overstep::new(0.0),
        });
        app.update();
        assert_eq!(app.world().get::<RemoteTimeline>(e).unwrap().now, TickInstant {
            tick: Tick(1),
            overstep: Overstep::new(0.0),
        });


    }
}



// #[cfg(test)]
// mod tests {
//     use super::*;
//     use crate::prelude::server::Replicate;
//     use crate::prelude::*;
//     use crate::tests::protocol::*;
//     use crate::tests::stepper::BevyStepper;
//     use core::time::Duration;
//
//     /// Check that after a big tick discrepancy between server/client, the client tick gets updated
//     /// to match the server tick
//     #[test]
//     fn test_sync_after_tick_wrap() {
//         let tick_duration = Duration::from_millis(10);
//         let mut stepper = BevyStepper::default();
//
//         // set time to end of wrapping
//         let new_tick = Tick(u16::MAX - 1000);
//         let new_time = WrappedTime::from_duration(tick_duration * (new_tick.0 as u32));
//
//         stepper
//             .server_app
//             .world_mut()
//             .resource_mut::<TimeManager>()
//             .set_current_time(new_time);
//         stepper
//             .server_app
//             .world_mut()
//             .resource_mut::<TickManager>()
//             .set_tick_to(new_tick);
//
//         let server_entity = stepper
//             .server_app
//             .world_mut()
//             .spawn((ComponentSyncModeFull(0.0), Replicate::default()))
//             .id();
//
//         // cross tick boundary
//         for i in 0..200 {
//             stepper.frame_step();
//         }
//         stepper
//             .server_app
//             .world_mut()
//             .entity_mut(server_entity)
//             .insert(ComponentSyncModeFull(1.0));
//         // dbg!(&stepper.server_tick());
//         // dbg!(&stepper.client_tick());
//         // dbg!(&stepper
//         //     .server_app
//         //     .world()
//         //     .get::<ComponentSyncModeFull>(server_entity));
//
//         // make sure the client receives the replication message
//         for i in 0..5 {
//             stepper.frame_step();
//         }
//
//         let client_entity = stepper
//             .client_app
//             .world()
//             .resource::<client::ConnectionManager>()
//             .replication_receiver
//             .remote_entity_map
//             .get_local(server_entity)
//             .unwrap();
//         assert_eq!(
//             stepper
//                 .client_app
//                 .world()
//                 .get::<ComponentSyncModeFull>(client_entity)
//                 .unwrap(),
//             &ComponentSyncModeFull(1.0)
//         );
//     }
// }
