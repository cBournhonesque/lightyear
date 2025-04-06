/*! Handles syncing the time between the client and the server
*/
use crate::plugin::{SyncPlugin, SyncedTimelinePlugin};
use crate::timeline::input::Input;
#[cfg(feature = "interpolation")]
use crate::timeline::interpolation::Interpolation;
use crate::timeline::remote::RemoteEstimate;
use crate::timeline::sync::SyncedTimeline;
use crate::timeline::{remote, DrivingTimeline};
use bevy::prelude::*;
use bevy::prelude::{Reflect, SystemSet};
use bevy::time::time_system;
use lightyear_core::prelude::NetworkTimelinePlugin;
use lightyear_core::timeline::Timeline;

// When a Client is created; we want to add a PredictedTimeline? InterpolatedTimeline?
//  or should we let the user do it?
// Systems we need:
//  - We want FixedUpdate to slow down if Predicted timeline slows down, because FixedUpdate is fundamentally
//      what decides
//  - we update
pub struct ClientPlugin;

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
        app.add_plugins(SyncPlugin);

        app.add_plugins(SyncedTimelinePlugin::<Input, RemoteEstimate>::default());

        #[cfg(feature = "interpolation")]
        app.add_plugins(SyncedTimelinePlugin::<Interpolation, RemoteEstimate>::default());

        app.add_plugins(NetworkTimelinePlugin::<RemoteEstimate>::default());

        app.add_observer(remote::update_remote_timeline);
        app.add_systems(First, remote::advance_remote_timeline.after(time_system));

        // TODO: should this be configurable?
        // the client will use the Input timeline as the driving timeline
        app.register_required_components::<Timeline<Input>, DrivingTimeline<Input>>();

        app.add_systems(Last, SyncPlugin::update_virtual_time::<Input>);
    }
}



#[cfg(test)]
mod tests {
    use super::*;
    use crate::prelude::server::Replicate;
    use crate::prelude::*;
    use crate::tests::protocol::*;
    use crate::tests::stepper::BevyStepper;
    use core::time::Duration;

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
