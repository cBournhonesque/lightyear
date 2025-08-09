use crate::SyncComponent;
use crate::interpolation_history::ConfirmedHistory;
use crate::registry::InterpolationRegistry;
use crate::timeline::InterpolationTimeline;
use bevy_ecs::{
    component::{Component, Mutable},
    entity::Entity,
    query::{With, Without},
    system::{Commands, Query, Res, Single},
};
use lightyear_core::prelude::{LocalTimeline, NetworkTimeline, Tick};
use lightyear_core::tick::TickDuration;
use lightyear_sync::prelude::client::IsSynced;
use tracing::trace;

// if we haven't received updates since UPDATE_INTERPOLATION_START_TICK_FACTOR * send_interval
// then we update the start_tick so that the interpolation looks good when we receive a new update
// - lower values (with a minimum of 1.0) will make the interpolation look better when we receive an update,
//   but will also make it more likely to have a wrong interpolation when we have packet loss
// - however we can combat packet loss by having a bigger delay
// TODO: this value should depend on jitter I think
const SEND_INTERVAL_TICK_FACTOR: f32 = 1.3;

// TODO: the inner fields are pub just for integration testing.
//  maybe put the test here?
// NOTE: there's not a strict need for this, it just makes the logic easier to follow
/// Component that will tract the values to interpolate between, as well as the interpolation ratio.
/// This is provided so that you can easily compute your own interpolation if you want to.
#[derive(Component, PartialEq, Debug)]
pub struct InterpolateStatus<C: Component> {
    /// start tick to interpolate from, along with value
    pub start: Option<(Tick, C)>,
    /// end tick to interpolate to, along with value
    pub end: Option<(Tick, C)>,
    /// current interpolation tick, which will belong to [start_tick, end_tick[
    pub current_tick: Tick,
    /// for more accurate interpolation, this is the fraction between [current_tick, current_tick + 1[
    pub current_overstep: f32,
}

impl<C: Component> InterpolateStatus<C> {
    pub fn interpolation_fraction(&self) -> Option<f32> {
        self.start.as_ref().and_then(|(start_tick, _)| {
            self.end.as_ref().map(|(end_tick, _)| {
                if *start_tick != *end_tick {
                    ((self.current_tick - *start_tick) as f32 + self.current_overstep)
                        / (*end_tick - *start_tick) as f32
                } else {
                    0.0
                }
            })
        })
    }
}

/// At the end of each frame, interpolate the components between the last 2 confirmed server states
/// Invariant: start_tick <= current_interpolate_tick + overstep < end_tick
pub(crate) fn update_interpolate_status<C: SyncComponent>(
    // TODO: handle multiple interpolation timelines
    // TODO: exclude host-server
    interpolation: Single<&InterpolationTimeline, With<IsSynced<InterpolationTimeline>>>,
    tick_duration: Res<TickDuration>,
    mut query: Query<(
        Entity,
        Option<&mut C>,
        &mut InterpolateStatus<C>,
        &mut ConfirmedHistory<C>,
    )>,
) {
    let kind = core::any::type_name::<C>();
    let timeline = interpolation.into_inner();

    // how many ticks between each interpolation
    let send_interval_delta_tick = (SEND_INTERVAL_TICK_FACTOR
        * timeline.remote_send_interval.as_secs_f32()
        / tick_duration.as_secs_f32())
    .ceil() as i16;

    let current_interpolate_tick = timeline.now().tick;
    let current_interpolate_overstep = timeline.now().overstep;
    for (entity, component, mut status, mut history) in query.iter_mut() {
        let mut start = status.start.take();
        let mut end = status.end.take();

        // if the interpolation tick is beyond the previous end tick,
        // we need to replace start with end, and clear end
        if let Some((end_tick, ref end_value)) = end
            && end_tick <= current_interpolate_tick
        {
            trace!(
                ?entity,
                ?end_tick,
                ?current_interpolate_tick,
                "interpolation is beyond previous end tick"
            );
            start.clone_from(&end);
            // TODO: this clone should be avoidable
            if let Some(mut component) = component {
                *component = end_value.clone();
            }
            end = None;
        }

        // TODO: do we need to call this if status.end is set? probably not because the updates are sequenced?

        // TODO: CAREFUL, we need to always leave a value in the history, so that we can compute future values?
        //  maybe not, because for interpolation we don't care about the value at a given specific tick

        // clear all values with a tick <= current_interpolate_tick, and get the last cleared value
        // (we need to call this even if status.start is set, because a new more recent server update could have been received)
        let new_start = history.pop_until_tick(current_interpolate_tick);
        if let Some((new_tick, _)) = new_start
            && start.as_ref().is_none_or(|(tick, _)| *tick <= new_tick)
        {
            trace!(
                    ?current_interpolate_tick,
                    old_start = ?start.as_ref().map(|(tick, _)| tick),
                    new_start = ?new_tick,
                    "found more recent tick between start and interpolation tick");
            start = new_start;
        }

        // get the next value immediately > current_interpolate_tick, but without popping
        // (we need to call this even if status.end is set, because a new more recent server update could have been received)
        if let Some((new_tick, _)) = history.peek()
            && end.as_ref().is_none_or(|(tick, _)| new_tick < *tick)
        {
            trace!("next value after current_interpolate_tick: {:?}", new_tick);
            // only pop if we actually put the value in end
            end = history.pop();
        }

        // If it's been too long since we received an update, reset the start tick to None
        // (so that we wait again until interpolation_tick is between two server updates)
        // otherwise the interpolation will seem weird because the start tick is very old
        // Only do this when end_tick is None, otherwise it could affect the currently running
        // interpolation
        if end.is_none() {
            let temp_start = core::mem::take(&mut start);
            if let Some((start_tick, _)) = temp_start {
                if current_interpolate_tick - start_tick < send_interval_delta_tick {
                    start = temp_start;
                } else {
                    trace!(
                        ?current_interpolate_tick,
                        ?start_tick,
                        ?send_interval_delta_tick,
                        "Reset the start_tick because it's been too long since we received an update"
                    );
                }
                // else (if it's been too long), reset the server tick to None
            }
        }

        trace!(
            ?entity,
            component = ?kind,
            ?send_interval_delta_tick,
            ?current_interpolate_tick,
            ?current_interpolate_overstep,
            // last_received_server_tick = ?connection.latest_received_server_tick(),
            start_tick = ?start.as_ref().map(|(tick, _)| tick),
            end_tick = ?end.as_ref().map(|(tick, _) | tick),
            "update_interpolate_status");
        status.start = start;
        status.end = end;
        status.current_tick = current_interpolate_tick;
        status.current_overstep = current_interpolate_overstep.value();
        if status.start.is_none() {
            trace!("no lerp start tick");
        }
        if status.end.is_none() {
            // warn!("no lerp end tick: might want to increase the interpolation delay");
        }
    }
}

/// Insert the component on the `Interpolated` entity.
/// We do not insert the component immediately on `Interpolated` when the component gets added on the `Confirmed` entity,
/// because then the component value would be constant (= to the starting value) until we get another component update,
/// and then it starts moving. This can be jarring if the server send rate is low (then for example a bullet is frozen for a bit
/// before it starts moving).
/// Instead we will insert the component after either:
/// - we have received 2 updates on the Confirmed entity (so we can interpolate between them)
/// - or at least SEND_INTERVAL_TICK_FACTOR * send_interval has passed. (this is to deal with the case where we only receive
///   one update; for example if we spawn the player and then they don't move. If we didn't do this
///   the interpolated entity would simply not appear)
pub(crate) fn insert_interpolated_component<C: SyncComponent>(
    component_registry: Res<InterpolationRegistry>,
    tick_duration: Res<TickDuration>,
    tick_manager: Single<(&LocalTimeline, &InterpolationTimeline)>,
    mut commands: Commands,
    mut query: Query<(Entity, &InterpolateStatus<C>), Without<C>>,
) {
    let (local_timeline, interpolation_timeline) = tick_manager.into_inner();
    let tick = local_timeline.tick();
    // how many ticks between each interpolation update (add 1 to roughly take the ceil)
    // TODO: use something more precise, with the interpolation overstep?
    let send_interval_delta_tick = (SEND_INTERVAL_TICK_FACTOR
        * interpolation_timeline.remote_send_interval.as_secs_f32()
        / tick_duration.as_secs_f32()) as i16
        + 1;
    for (entity, status) in query.iter_mut() {
        trace!("checking if we need to insert the component on the Interpolated entity");
        let mut entity_commands = commands.entity(entity);
        // NOTE: it is possible that we reach start_tick when end_tick is not set
        if let Some((start_tick, start_value)) = &status.start {
            trace!(is_end = ?status.end.is_some(), "start tick exists, checking if we need to insert the component");
            // we have two updates!, add the component
            match &status.end {
                Some((end_tick, end_value)) => {
                    assert!(status.current_tick < *end_tick);
                    assert_ne!(start_tick, end_tick);
                    trace!("insert interpolated comp value because we have 2 updates");
                    let t = status.interpolation_fraction().unwrap();
                    let value =
                        component_registry.interpolate(start_value.clone(), end_value.clone(), t);
                    entity_commands.insert(value.clone());
                }
                _ => {
                    // we only have one update, but enough time has passed that we should add the component anyway
                    if tick - *start_tick >= send_interval_delta_tick {
                        trace!("insert interpolated comp value because enough time has passed");
                        entity_commands.insert(start_value.clone());
                    }
                }
            }
        }
    }
}

/// Update the component value on the Interpolate entity
pub(crate) fn interpolate<C: Component<Mutability = Mutable> + Clone>(
    component_registry: Res<InterpolationRegistry>,
    mut query: Query<(&mut C, &InterpolateStatus<C>)>,
) {
    for (mut component, status) in query.iter_mut() {
        // NOTE: it is possible that we reach start_tick when end_tick is not set
        if let Some((start_tick, start_value)) = &status.start
            && let Some((end_tick, end_value)) = &status.end
        {
            trace!(?start_tick, interpolate_tick=?status.current_tick, ?end_tick, "doing interpolation!");
            assert!(status.current_tick < *end_tick);
            if start_tick != end_tick {
                let t = status.interpolation_fraction().unwrap();
                let value =
                    component_registry.interpolate(start_value.clone(), end_value.clone(), t);
                *component = value.clone();
            } else {
                *component = start_value.clone();
            }
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
