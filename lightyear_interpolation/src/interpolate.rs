use crate::interpolation_history::ConfirmedHistory;
use crate::registry::InterpolationRegistry;
use crate::timeline::InterpolationTimeline;
use bevy_ecs::component::Mutable;
use bevy_ecs::prelude::Has;
use bevy_ecs::prelude::*;
use bevy_utils::prelude::DebugName;
use lightyear_core::prelude::NetworkTimeline;
use lightyear_core::tick::{Tick, TickDuration};
use lightyear_sync::prelude::client::IsSynced;
#[allow(unused_imports)]
use tracing::{info, trace};

// if we haven't received updates since UPDATE_INTERPOLATION_START_TICK_FACTOR * send_interval
// then we update the start_tick so that the interpolation looks good when we receive a new update
// - lower values (with a minimum of 1.0) will make the interpolation look better when we receive an update,
//   but will also make it more likely to have a wrong interpolation when we have packet loss
// - however we can combat packet loss by having a bigger delay
// TODO: this value should depend on jitter I think
const SEND_INTERVAL_TICK_FACTOR: f32 = 1.3;

/// Compute the interpolation fraction
pub fn interpolation_fraction(start: Tick, end: Tick, current: Tick, overstep: f32) -> f32 {
    ((current - start) as f32 + overstep) / (end - start) as f32
}

/// Update the ConfirmedHistory so that interpolation can simply interpolate between the last 2 updates.
pub(crate) fn update_confirmed_history<C: Component + Clone>(
    // TODO: handle multiple interpolation timelines
    // TODO: exclude host-server
    interpolation: Single<&InterpolationTimeline, With<IsSynced<InterpolationTimeline>>>,
    tick_duration: Res<TickDuration>,
    // we don't insert the component immediately, instead we wait for:
    // - either 2 updates, so that we can interpolate between them
    // - or enough time has passed since the initial update
    mut query: Query<(Entity, &mut ConfirmedHistory<C>, Has<C>)>,
    mut commands: Commands,
) {
    let timeline = interpolation.into_inner();

    // how many ticks between each interpolation
    let send_interval_delta_tick = (SEND_INTERVAL_TICK_FACTOR
        * timeline.remote_send_interval.as_secs_f32()
        / tick_duration.as_secs_f32())
    .ceil() as i16;

    let current_interpolate_tick = timeline.now().tick;
    for (entity, mut history, present) in query.iter_mut() {
        // the ConfirmedHistory contains an ordered list (from oldest to most recent) of Confirmed component updates to interpolate between
        // The component must always be interpolating between the oldest and the second oldest values in the history.
        // History: H1...X...H2.....H3

        // We enforce this like so:
        // - If we have 2 older history values than the current tick, we pop the last value
        //   History: H1....H2..X..H3  -> pop H1
        if let Some((history_tick, end_value)) = history.end() {
            // we have 2 updates, we can start interpolating!
            if !present {
                info!(
                    ?entity, ?history_tick, ?current_interpolate_tick,
                    "insert interpolated comp value because we have 2 values to interpolate between. Kind = {:?}", DebugName::type_name::<C>()
                );
                // we can insert the end_value because:
                // - if H1..X...H2, we will do interpolation right after
                // - if H1...H2..X, H2 is a good starting point for the interpolation
                commands.entity(entity).insert(end_value.clone());
            }
            if current_interpolate_tick >= history_tick {
                history.pop();
            }
        }

        // If it's been too long since we last received an update; we pop the value from the history and
        // re-insert it as the current interpolation tick.
        // Otherwise we would be interpolating from a very old value, which would look strange.
        //   History: H1.....................................X..H2...H3 -> H1.X...H2..H3
        // We will then wait to have 2 new values before we can interpolate.
        if history.len() == 1
            && let Some((history_tick, val)) = history.start()
            && (current_interpolate_tick - history_tick) >= send_interval_delta_tick
        {
            if !present {
                info!(
                    ?entity, ?history_tick, ?current_interpolate_tick,
                    "insert interpolated comp value because we enough time has passed. Kind = {:?}", DebugName::type_name::<C>()
                );
                commands.entity(entity).insert(val.clone());
            }

            trace!(
                ?current_interpolate_tick,
                ?history_tick,
                ?send_interval_delta_tick,
                "Reset the start_tick because it's been too long since we received an update"
            );
            let (_, val) = history.pop().unwrap();
            // TODO: the correct behaviour would be to know the exact tick at which the component started getting updated
            //  so that we know exactly which tick to interpolate from!
            // we reset the value to a more recent tick so that the interpolation is done between two close values.
            history.push(current_interpolate_tick, val);
        }

        // It is possible that the interpolation_tick is early compared to the two updates
        // (for example we received an update H, then no update for a while so we removed it from the history, and then
        //  we receive two updates ahead of the interpolation_tick)
        // X...H1...H2
        // In which case the interpolation should not run.

        // TODO: if we don't have 2 values to interpolate between, we should extrapolate
        //   History: H1....X...  -> extrapolate X
    }
}

/// Apply interpolation for the component
pub(crate) fn interpolate<C: Component<Mutability = Mutable> + Clone>(
    interpolation_registry: Res<InterpolationRegistry>,
    timeline: Single<&InterpolationTimeline, With<IsSynced<InterpolationTimeline>>>,
    mut query: Query<(&mut C, &ConfirmedHistory<C>)>,
) {
    let interpolation_tick = timeline.tick();
    let interpolation_overstep = timeline.overstep().value();
    for (mut component, history) in query.iter_mut() {
        if let Some(interpolated) = history.interpolate(
            interpolation_tick,
            interpolation_overstep,
            interpolation_registry.as_ref(),
        ) {
            *component = interpolated;
        }
    }
}

// #[cfg(test)]
// mod tests {
//     #![allow(unused_imports)]
//     #![allow(unused_variables)]
//     #![allow(dead_code)]
//
//     use std::net::SocketAddr;
//     use std::str::FromStr;
//     use bevy::utils::{Duration, Instant};
//
//     use bevy::log::LogPlugin;
//     use bevy::prelude::*;
//     use bevy::time::TimeUpdateStrategy;
//     use bevy::{DefaultPlugins, MinimalPlugins};
//     use tracing::{debug, info};
//     use tracing_subscriber::fmt::format::FmtSpan;
//
//     use crate::_reexport::*;
//     use crate::prelude::client::*;
//     use crate::prelude::*;
//     use crate::tests::protocol::*;
//     use crate::tests::stepper::{BevyStepper};
//
//     fn setup() -> (BevyStepper, Entity, Entity) {
//         let frame_duration = Duration::from_millis(10);
//         let tick_duration = Duration::from_millis(10);
//         let shared_config = SharedConfig {
//             enable_replication: false,
//             tick: TickConfig::new(tick_duration),
//             ..Default::default()
//         };
//         let link_conditioner = LinkConditionerConfig {
//             incoming_latency: Duration::from_millis(40),
//             incoming_jitter: Duration::from_millis(5),
//             incoming_loss: 0.05,
//         };
//         let sync_config = SyncConfig::default().speedup_factor(1.0);
//         let prediction_config = PredictionConfig::default().disable(true);
//         let interpolation_delay = Duration::from_millis(100);
//         let interpolation_config = InterpolationConfig::default().with_delay(InterpolationDelay {
//             min_delay: interpolation_delay,
//             send_interval_ratio: 0.0,
//         });
//         let mut stepper = BevyStepper::new(
//             shared_config,
//             sync_config,
//             prediction_config,
//             interpolation_config,
//             link_conditioner,
//             frame_duration,
//         );
//         stepper.init();
//
//         // Create a confirmed entity on the server
//         let server_entity = stepper
//             .server_app
//             .world_mut()
//             .spawn((Component1(0.0), ShouldBeInterpolated))
//             .id();
//
//         // Set the latest received server tick
//         let confirmed_tick = stepper.client_app().world_mut().resource_mut::<ClientConnectionManager>()
//             .replication_receiver
//             .remote_entity_map
//             .get_confirmed_tick(confirmed_entity)
//             .unwrap();
//
//         // Tick once
//         stepper.frame_step();
//         let tick = stepper.client_tick();
//         let interpolated = stepper
//             .client_app
//             .world()//             .get::<Confirmed>(confirmed)
//             .unwrap()
//             .interpolated
//             .unwrap();
//
//         assert_eq!(
//             stepper
//                 .client_app
//                 .world()//                 .get::<Component1>(confirmed)
//                 .unwrap(),
//             &Component1(0.0)
//         );
//
//         // check that the interpolated entity got spawned
//         assert_eq!(
//             stepper
//                 .client_app
//                 .world()//                 .get::<Interpolated>(interpolated)
//                 .unwrap()
//                 .confirmed_entity,
//             confirmed
//         );
//
//         // check that the component history got created and is empty
//         let history = ConfirmedHistory::<Component1>::new();
//         assert_eq!(
//             stepper
//                 .client_app
//                 .world()//                 .get::<ConfirmedHistory<Component1>>(interpolated)
//                 .unwrap(),
//             &history,
//         );
//         // check that the confirmed component got replicated
//         assert_eq!(
//             stepper
//                 .client_app
//                 .world()//                 .get::<Component1>(interpolated)
//                 .unwrap(),
//             &Component1(0.0)
//         );
//         // check that the interpolate status got updated
//         let interpolation_tick = stepper.interpolation_tick();
//         assert_eq!(
//             stepper
//                 .client_app
//                 .world()//                 .get::<InterpolateStatus<Component1>>(interpolated)
//                 .unwrap(),
//             &InterpolateStatus::<Component1> {
//                 start: None,
//                 end: (tick, Component1(0.0)).into(),
//                 current: interpolation_tick,
//             }
//         );
//         (stepper, confirmed, interpolated)
//     }
//
//     // Test interpolation
//     #[test]
//     fn test_interpolation() -> anyhow::Result<()> {
//         let (mut stepper, confirmed, interpolated) = setup();
//         let start_tick = stepper.client_tick();
//         // reach interpolation start tick
//         stepper.frame_step();
//         stepper.frame_step();
//
//         // check that the interpolate status got updated (end becomes start)
//         assert_eq!(
//             stepper
//                 .client_app
//                 .world()//                 .get::<InterpolateStatus<Component1>>(interpolated)
//                 .unwrap(),
//             &InterpolateStatus::<Component1> {
//                 start: (Tick(0), Component1(0.0)).into(),
//                 end: None,
//                 current: Tick(3),
//                 // current: Tick(3) - interpolation_tick_delay,
//             }
//         );
//
//         // receive server update
//         // stepper
//         //     .client_mut()
//         //     .set_latest_received_server_tick(Tick(2));
//         stepper
//             .client_app
//             .world_mut()
//             .get_entity_mut(confirmed)
//             .unwrap()
//             .get_mut::<Component1>()
//             .unwrap()
//             .0 = 2.0;
//
//         stepper.frame_step();
//         // check that interpolation is working correctly
//         assert_eq!(
//             stepper
//                 .client_app
//                 .world()//                 .get::<InterpolateStatus<Component1>>(interpolated)
//                 .unwrap(),
//             &InterpolateStatus::<Component1> {
//                 start: (Tick(0), Component1(0.0)).into(),
//                 end: (Tick(2), Component1(2.0)).into(),
//                 current: Tick(4),
//                 // current: Tick(4) - interpolation_tick_delay,
//             }
//         );
//         assert_eq!(
//             stepper
//                 .client_app
//                 .world()//                 .get::<Component1>(interpolated)
//                 .unwrap(),
//             &Component1(1.0)
//         );
//         stepper.frame_step();
//         assert_eq!(
//             stepper
//                 .client_app
//                 .world()//                 .get::<InterpolateStatus<Component1>>(interpolated)
//                 .unwrap(),
//             &InterpolateStatus::<Component1> {
//                 start: (Tick(2), Component1(2.0)).into(),
//                 end: None,
//                 current: Tick(5),
//                 // current: Tick(5) - interpolation_tick_delay,
//             }
//         );
//         assert_eq!(
//             stepper
//                 .client_app
//                 .world()//                 .get::<Component1>(interpolated)
//                 .unwrap(),
//             &Component1(2.0)
//         );
//         Ok(())
//     }
//
//     // We are in the situation: S1 < I
//     // where S1 is a confirmed ticks, and I is the interpolated tick
//     // and we receive S1 < S2 < I
//     // Then we should now start interpolating from S2
//     #[test]
//     fn test_received_more_recent_start() -> anyhow::Result<()> {
//         let (mut stepper, confirmed, interpolated) = setup();
//
//         // reach interpolation start tick
//         stepper.frame_step();
//         stepper.frame_step();
//         stepper.frame_step();
//         stepper.frame_step();
//         assert_eq!(stepper.client_tick(), Tick(5));
//
//         // receive server update
//         // stepper
//         //     .client_mut()
//         //     .set_latest_received_server_tick(Tick(1));
//         stepper
//             .client_app
//             .world_mut()
//             .get_entity_mut(confirmed)
//             .unwrap()
//             .get_mut::<Component1>()
//             .unwrap()
//             .0 = 1.0;
//
//         stepper.frame_step();
//         // check the status uses the more recent server update
//         assert_eq!(
//             stepper
//                 .client_app
//                 .world()//                 .get::<InterpolateStatus<Component1>>(interpolated)
//                 .unwrap(),
//             &InterpolateStatus::<Component1> {
//                 start: (Tick(1), Component1(1.0)).into(),
//                 end: None,
//                 current: Tick(6),
//                 // current: Tick(6) - interpolation_tick_delay,
//             }
//         );
//         Ok(())
//     }
// }
