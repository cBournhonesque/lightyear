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

/// Maintain the ConfirmedHistory so that `interpolate()` always sees the right anchors.
///
/// Goal: keep the history at `[behind, newest]` while updates flow, where `behind` is the
/// most recent keyframe at or before the current interpolation tick. Under packet loss this
/// means `interpolate()` blends between the freshest pair of confirmed values that bracket
/// the current tick (or clamps at `newest` when we momentarily run out of forward anchors),
/// instead of falling back to a single-keyframe rebase that snaps on every gap.
///
/// We only collapse to a single keyframe when the entity has been idle long enough that the
/// next update is genuinely a fresh start, in which case we rebase the lone keyframe to the
/// current tick so a future update interpolates from "now" rather than from a stale tick.
pub(crate) fn update_confirmed_history<C: Component + Clone>(
    // TODO: handle multiple interpolation timelines
    // TODO: exclude host-server
    interpolation: Single<&InterpolationTimeline, With<IsSynced<InterpolationTimeline>>>,
    tick_duration: Res<TickDuration>,
    mut query: Query<(Entity, &mut ConfirmedHistory<C>, Has<C>)>,
    mut commands: Commands,
) {
    let timeline = interpolation.into_inner();

    let send_interval_delta_tick = (SEND_INTERVAL_TICK_FACTOR
        * timeline.remote_send_interval.as_secs_f32()
        / tick_duration.as_secs_f32())
    .ceil() as i16;

    let current_interpolate_tick = timeline.now().tick();
    for (entity, mut history, present) in query.iter_mut() {
        // Drop keyframes older than the most recent one at or before the current tick: we
        // only need one anchor "behind" the current tick to interpolate from.
        while history.len() >= 2
            && history
                .get_nth(1)
                .is_some_and(|(t, _)| t <= current_interpolate_tick)
        {
            history.pop();
        }

        // Seed the component on the first sync so the entity has something to render before
        // `interpolate()` runs. Choose `newest` so a brand-new entity snaps to the freshest
        // confirmed value rather than a stale tail.
        if !present && let Some((_, value)) = history.newest() {
            commands.entity(entity).insert(value.clone());
        }

        // Idle rebase: if only one keyframe remains and the current tick has drifted past it
        // by more than the expected send interval, treat the entity as idle and rebase the
        // keyframe to "now". Without this, the next incoming update would create a stale-tick
        // pair and produce a long catch-up blend that looks like a snap.
        if history.len() == 1
            && let Some((newest_tick, _)) = history.newest()
            && (current_interpolate_tick - newest_tick) >= send_interval_delta_tick
        {
            trace!(
                ?entity,
                ?newest_tick,
                ?current_interpolate_tick,
                "rebase idle keyframe to current tick. Kind = {:?}",
                DebugName::type_name::<C>()
            );
            let (_, value) = history.pop().unwrap();
            history.push(current_interpolate_tick, value);
        }
    }
}

/// Apply interpolation for the component
pub(crate) fn interpolate<C: Component<Mutability = Mutable> + Clone>(
    interpolation_registry: Res<InterpolationRegistry>,
    timeline: Single<&InterpolationTimeline, With<IsSynced<InterpolationTimeline>>>,
    mut query: Query<(&mut C, &ConfirmedHistory<C>)>,
) {
    let interpolation_tick = timeline.tick();
    let interpolation_overstep = timeline.overstep().to_f32();
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
