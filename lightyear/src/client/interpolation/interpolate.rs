use bevy::prelude::{Component, Query, ResMut};
use tracing::{info, trace, warn};

use crate::client::components::{ComponentSyncMode, SyncComponent};
use crate::client::interpolation::interpolation_history::ConfirmedHistory;
use crate::client::interpolation::InterpolatedComponent;
use crate::client::resource::Client;
use crate::protocol::Protocol;
use crate::shared::tick_manager::Tick;

// TODO: the inner fields are pub just for integration testing.
//  maybe put the test here?
// NOTE: there's not a strict need for this, it just makes the logic easier to follow
#[derive(Component, PartialEq, Debug)]
pub struct InterpolateStatus<C: SyncComponent> {
    /// start tick to interpolate from, along with value
    pub start: Option<(Tick, C)>,
    /// end tick to interpolate to, along with value
    pub end: Option<(Tick, C)>,
    /// current interpolation tick
    pub current: Tick,
}

/// At the end of each frame, interpolate the components between the last 2 confirmed server states
pub(crate) fn update_interpolate_status<C: SyncComponent, P: Protocol>(
    client: ResMut<Client<P>>,
    mut query: Query<(&mut C, &mut InterpolateStatus<C>, &mut ConfirmedHistory<C>)>,
) {
    if C::mode() != ComponentSyncMode::Full {
        return;
    }
    if !client.is_synced() {
        return;
    }
    let current_interpolate_tick = client.interpolation_tick();
    for (mut component, mut status, mut history) in query.iter_mut() {
        let mut start = status.start.take();
        let mut end = status.end.take();

        // if the interpolation tick is beyond the previous end tick,
        // we need to replace start with end, and clear end
        if let Some((end_tick, ref end_value)) = end {
            if end_tick <= current_interpolate_tick {
                start = end.clone();
                // TODO: this clone should be avoidable
                *component = end_value.clone();
                end = None;
            }
        }

        // clear all values with a tick <= current_interpolate_tick, and get the last cleared value
        // (we need to call this even if status.start is set, because a new more recent server update could have been received)
        let new_start = history.pop_until_tick(current_interpolate_tick);
        if let Some((new_tick, _)) = new_start {
            if start.as_ref().map_or(true, |(tick, _)| *tick < new_tick) {
                start = new_start;
            }
        }

        // get the next value immediately > current_interpolate_tick, but without popping
        // (we need to call this even if status.end is set, because a new more recent server update could have been received)
        if let Some((new_tick, _)) = history.peek() {
            if end.as_ref().map_or(true, |(tick, _)| new_tick < *tick) {
                // only pop if we actually put the value in end
                end = history.pop();
            }
        }

        trace!(?current_interpolate_tick,
            last_received_server_tick = ?client.latest_received_server_tick(),
            start_tick = ?start.as_ref().map(|(tick, _)| tick),
            end_tick = ?end.as_ref().map(|(tick, _) | tick),
            "update_interpolate_status");
        status.start = start;
        status.end = end;
        status.current = current_interpolate_tick;
        if status.start.is_none() {
            trace!("no lerp start tick");
        }
        if status.end.is_none() {
            // warn!("no lerp end tick: might want to increase the interpolation delay");
        }
    }
}

pub(crate) fn interpolate<C: InterpolatedComponent>(
    mut query: Query<(&mut C, &InterpolateStatus<C>)>,
) {
    for (mut component, status) in query.iter_mut() {
        if let (Some((start_tick, start_value)), Some((end_tick, end_value))) =
            (&status.start, &status.end)
        {
            // info!(?start_tick, ?end_tick, "doing interpolation!");
            if start_tick != end_tick {
                let t = (status.current - *start_tick) as f32 / (*end_tick - *start_tick) as f32;
                *component = C::lerp(start_value.clone(), end_value.clone(), t);
            } else {
                *component = start_value.clone();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    // #![allow(unused_imports)]
    // #![allow(unused_variables)]
    // #![allow(dead_code)]
    //
    // use std::net::SocketAddr;
    // use std::str::FromStr;
    // use std::time::{Duration, Instant};
    //
    // use bevy::log::LogPlugin;
    // use bevy::prelude::{
    //     App, Commands, Entity, EventReader, FixedUpdate, IntoSystemConfigs, PluginGroup, Query, Real,
    //     Res, ResMut, Startup, Time, With,
    // };
    // use bevy::time::TimeUpdateStrategy;
    // use bevy::winit::WinitPlugin;
    // use bevy::{DefaultPlugins, MinimalPlugins};
    // use lightyear::client::components::Confirmed;
    // use tracing::{debug, info};
    // use tracing_subscriber::fmt::format::FmtSpan;
    //
    // use lightyear::_reexport::*;
    // use lightyear::prelude::client::*;
    // use lightyear::prelude::*;
    // use lightyear_tests::protocol::{protocol, Channel2, Component1, Component2, MyInput, MyProtocol};
    // use lightyear_tests::stepper::{BevyStepper, Step};
    //
    // fn setup() -> (BevyStepper, Entity, Entity, u16) {
    //     let frame_duration = Duration::from_millis(10);
    //     let tick_duration = Duration::from_millis(10);
    //     let shared_config = SharedConfig {
    //         enable_replication: false,
    //         tick: TickConfig::new(tick_duration),
    //         ..Default::default()
    //     };
    //     let link_conditioner = LinkConditionerConfig {
    //         incoming_latency: Duration::from_millis(40),
    //         incoming_jitter: Duration::from_millis(5),
    //         incoming_loss: 0.05,
    //     };
    //     let sync_config = SyncConfig::default().speedup_factor(1.0);
    //     let prediction_config = PredictionConfig::default().disable(true);
    //     let interpolation_delay = Duration::from_millis(100);
    //     let interpolation_config =
    //         InterpolationConfig::default().with_delay(InterpolationDelay::Delay(interpolation_delay));
    //     let mut stepper = BevyStepper::new(
    //         shared_config,
    //         sync_config,
    //         prediction_config,
    //         interpolation_config,
    //         link_conditioner,
    //         frame_duration,
    //     );
    //     stepper.client_mut().set_synced();
    //
    //     // Create a confirmed entity
    //     let confirmed = stepper
    //         .client_app
    //         .world
    //         .spawn((Component1(0.0), ShouldBeInterpolated))
    //         .id();
    //
    //     // Tick once
    //     stepper.frame_step();
    //     assert_eq!(stepper.client().tick(), Tick(1));
    //     let interpolated = stepper
    //         .client_app
    //         .world
    //         .get::<Confirmed>(confirmed)
    //         .unwrap()
    //         .interpolated
    //         .unwrap();
    //
    //     assert_eq!(
    //         stepper
    //             .client_app
    //             .world
    //             .get::<Component1>(confirmed)
    //             .unwrap(),
    //         &Component1(0.0)
    //     );
    //
    //     // check that the interpolated entity got spawned
    //     assert_eq!(
    //         stepper
    //             .client_app
    //             .world
    //             .get::<Interpolated>(interpolated)
    //             .unwrap()
    //             .confirmed_entity,
    //         confirmed
    //     );
    //
    //     // check that the component history got created and is empty
    //     let history = ConfirmedHistory::<Component1>::new();
    //     assert_eq!(
    //         stepper
    //             .client_app
    //             .world
    //             .get::<ConfirmedHistory<Component1>>(interpolated)
    //             .unwrap(),
    //         &history,
    //     );
    //     // check that the confirmed component got replicated
    //     assert_eq!(
    //         stepper
    //             .client_app
    //             .world
    //             .get::<Component1>(interpolated)
    //             .unwrap(),
    //         &Component1(0.0)
    //     );
    //     // check that the interpolate status got updated
    //     assert_eq!(
    //         stepper
    //             .client_app
    //             .world
    //             .get::<InterpolateStatus<Component1>>(interpolated)
    //             .unwrap(),
    //         &InterpolateStatus::<Component1> {
    //             start: None,
    //             end: (Tick(0), Component1(0.0)).into(),
    //             current: Tick(1) - interpolation_tick_delay,
    //         }
    //     );
    //     (stepper, confirmed, interpolated, interpolation_tick_delay)
    // }
    //
    // // Test interpolation
    // #[test]
    // fn test_interpolation() -> anyhow::Result<()> {
    //     let (mut stepper, confirmed, interpolated, interpolation_tick_delay) = setup();
    //     // reach interpolation start tick
    //     stepper.frame_step();
    //     stepper.frame_step();
    //     // check that the interpolate status got updated (end becomes start)
    //     assert_eq!(
    //         stepper
    //             .client_app
    //             .world
    //             .get::<InterpolateStatus<Component1>>(interpolated)
    //             .unwrap(),
    //         &InterpolateStatus::<Component1> {
    //             start: (Tick(0), Component1(0.0)).into(),
    //             end: None,
    //             current: Tick(3) - interpolation_tick_delay,
    //         }
    //     );
    //
    //     // receive server update
    //     stepper
    //         .client_mut()
    //         .set_latest_received_server_tick(Tick(2));
    //     stepper
    //         .client_app
    //         .world
    //         .get_entity_mut(confirmed)
    //         .unwrap()
    //         .get_mut::<Component1>()
    //         .unwrap()
    //         .0 = 2.0;
    //
    //     stepper.frame_step();
    //     // check that interpolation is working correctly
    //     assert_eq!(
    //         stepper
    //             .client_app
    //             .world
    //             .get::<InterpolateStatus<Component1>>(interpolated)
    //             .unwrap(),
    //         &InterpolateStatus::<Component1> {
    //             start: (Tick(0), Component1(0.0)).into(),
    //             end: (Tick(2), Component1(2.0)).into(),
    //             current: Tick(4) - interpolation_tick_delay,
    //         }
    //     );
    //     assert_eq!(
    //         stepper
    //             .client_app
    //             .world
    //             .get::<Component1>(interpolated)
    //             .unwrap(),
    //         &Component1(1.0)
    //     );
    //     stepper.frame_step();
    //     assert_eq!(
    //         stepper
    //             .client_app
    //             .world
    //             .get::<InterpolateStatus<Component1>>(interpolated)
    //             .unwrap(),
    //         &InterpolateStatus::<Component1> {
    //             start: (Tick(2), Component1(2.0)).into(),
    //             end: None,
    //             current: Tick(5) - interpolation_tick_delay,
    //         }
    //     );
    //     assert_eq!(
    //         stepper
    //             .client_app
    //             .world
    //             .get::<Component1>(interpolated)
    //             .unwrap(),
    //         &Component1(2.0)
    //     );
    //     Ok(())
    // }
    //
    // // We are in the situation: S1 < I
    // // where S1 is a confirmed ticks, and I is the interpolated tick
    // // and we receive S1 < S2 < I
    // // Then we should now start interpolating from S2
    // #[test]
    // fn test_received_more_recent_start() -> anyhow::Result<()> {
    //     let (mut stepper, confirmed, interpolated, interpolation_tick_delay) = setup();
    //
    //     // reach interpolation start tick
    //     stepper.frame_step();
    //     stepper.frame_step();
    //     stepper.frame_step();
    //     stepper.frame_step();
    //     assert_eq!(stepper.client().tick(), Tick(5));
    //
    //     // receive server update
    //     stepper
    //         .client_mut()
    //         .set_latest_received_server_tick(Tick(1));
    //     stepper
    //         .client_app
    //         .world
    //         .get_entity_mut(confirmed)
    //         .unwrap()
    //         .get_mut::<Component1>()
    //         .unwrap()
    //         .0 = 1.0;
    //
    //     stepper.frame_step();
    //     // check the status uses the more recent server update
    //     assert_eq!(
    //         stepper
    //             .client_app
    //             .world
    //             .get::<InterpolateStatus<Component1>>(interpolated)
    //             .unwrap(),
    //         &InterpolateStatus::<Component1> {
    //             start: (Tick(1), Component1(1.0)).into(),
    //             end: None,
    //             current: Tick(6) - interpolation_tick_delay,
    //         }
    //     );
    //     Ok(())
    // }
}
