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

/// Maintain the confirmed-history anchors used for interpolation.
///
/// The goal is to keep the freshest bracketing pair while updates are flowing,
/// then converge to the newest confirmed value when updates stop arriving.
pub(crate) fn update_confirmed_history<C: Component + Clone>(
    // TODO: handle multiple interpolation timelines
    // TODO: exclude host-server
    interpolation: Single<&InterpolationTimeline, With<IsSynced<InterpolationTimeline>>>,
    tick_duration: Res<TickDuration>,
    mut query: Query<(Entity, &mut ConfirmedHistory<C>, Has<C>)>,
    mut commands: Commands,
) {
    let timeline = interpolation.into_inner();

    // how many ticks between each interpolation
    let send_interval_delta_tick = (SEND_INTERVAL_TICK_FACTOR
        * timeline.remote_send_interval.as_secs_f32()
        / tick_duration.as_secs_f32())
    .ceil() as i16;

    let current_interpolate_tick = timeline.now().tick();
    for (entity, mut history, present) in query.iter_mut() {
        // Smart drain: only pop when there are 3+ keyframes and the second-oldest
        // has already been passed. This keeps a [behind, newest] pair alive during
        // short loss gaps instead of collapsing immediately to a single keyframe.
        while history.len() >= 3
            && history
                .get_nth(1)
                .is_some_and(|(t, _)| t <= current_interpolate_tick)
        {
            history.pop();
        }

        // Seed the component as soon as we have interpolation state. If we already
        // have two anchors, the second-oldest is the best initial visible value;
        // otherwise use the single available anchor.
        if !present && let Some((_, value)) = history.end().or(history.start()) {
            commands.entity(entity).insert(value.clone());
        }

        // If the newest update is stale, collapse the history to a single anchor at
        // the current interpolation tick and write the newest confirmed value
        // directly. This guarantees convergence on the latest authoritative state
        // once updates stop arriving.
        let idle_value = match history.newest() {
            Some((newest_tick, value))
                if (current_interpolate_tick - newest_tick) >= send_interval_delta_tick =>
            {
                Some(value.clone())
            }
            _ => None,
        };
        if let Some(value) = idle_value {
            trace!(
                ?entity,
                ?current_interpolate_tick,
                "rebase idle keyframe. Kind = {:?}",
                DebugName::type_name::<C>()
            );
            while history.pop().is_some() {}
            // TODO: the correct behaviour would be to know the exact tick at which the
            // component started getting updated so that we know exactly which tick to
            // interpolate from. Using `current_interpolate_tick` here is a proxy.
            history.push(current_interpolate_tick, value.clone());
            commands.entity(entity).insert(value);
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
#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::InterpolationRegistry;
    use bevy_app::{App, Update};
    use bevy_ecs::component::Component;
    use lightyear_core::time::TickInstant;

    #[derive(Component, Clone, Debug, PartialEq)]
    struct TestComp(f32);

    fn lerp(start: TestComp, end: TestComp, t: f32) -> TestComp {
        TestComp(start.0 + (end.0 - start.0) * t)
    }

    fn setup_app(current_tick: Tick, send_interval_ms: u64) -> App {
        let mut app = App::new();
        app.world_mut()
            .insert_resource(TickDuration(core::time::Duration::from_millis(10)));

        let mut timeline = InterpolationTimeline::default();
        timeline.set_now(TickInstant::from(current_tick));
        timeline.remote_send_interval = core::time::Duration::from_millis(send_interval_ms);
        app.world_mut()
            .spawn((timeline, IsSynced::<InterpolationTimeline>::default()));
        app
    }

    #[test]
    fn update_confirmed_history_converges_to_latest_confirmed_value_when_idle() {
        let mut app = setup_app(Tick(30), 40);
        app.add_systems(Update, update_confirmed_history::<TestComp>);

        let entity = app.world_mut().spawn(TestComp(9.5)).id();
        let mut history = ConfirmedHistory::<TestComp>::default();
        history.push(Tick(10), TestComp(0.0));
        history.push(Tick(20), TestComp(10.0));
        app.world_mut().entity_mut(entity).insert(history);

        app.update();

        let component = app.world().get::<TestComp>(entity).unwrap();
        let history = app
            .world()
            .get::<ConfirmedHistory<TestComp>>(entity)
            .unwrap();
        assert_eq!(component, &TestComp(10.0));
        assert_eq!(history.len(), 1);
        assert_eq!(
            history.start().map(|(t, v)| (t, v.clone())),
            Some((Tick(30), TestComp(10.0)))
        );
    }

    #[test]
    fn update_confirmed_history_keeps_bracketing_pair_during_loss_gap() {
        let mut app = setup_app(Tick(25), 40);
        app.add_systems(
            Update,
            (
                update_confirmed_history::<TestComp>,
                interpolate::<TestComp>,
            )
                .chain(),
        );
        let mut registry = InterpolationRegistry::default();
        registry.set_interpolation::<TestComp>(lerp);
        app.world_mut().insert_resource(registry);

        let entity = app.world_mut().spawn(TestComp(999.0)).id();
        let mut history = ConfirmedHistory::<TestComp>::default();
        history.push(Tick(10), TestComp(0.0));
        history.push(Tick(20), TestComp(10.0));
        history.push(Tick(30), TestComp(20.0));
        app.world_mut().entity_mut(entity).insert(history);

        app.update();

        let component = app.world().get::<TestComp>(entity).unwrap();
        let history = app
            .world()
            .get::<ConfirmedHistory<TestComp>>(entity)
            .unwrap();
        assert_eq!(history.len(), 2);
        assert_eq!(
            history.start().map(|(t, v)| (t, v.clone())),
            Some((Tick(20), TestComp(10.0)))
        );
        assert_eq!(
            history.end().map(|(t, v)| (t, v.clone())),
            Some((Tick(30), TestComp(20.0)))
        );
        assert_eq!(component, &TestComp(15.0));
    }
}
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
