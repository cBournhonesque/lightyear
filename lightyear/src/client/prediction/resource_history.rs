//! There's a lot of overlap with `client::prediction_history` because resources are components in ECS so rollback is going to look similar.
use crate::prelude::{HistoryBuffer, HistoryState, TickManager};
use bevy::prelude::*;

use super::rollback::Rollback;
use crate::shared::tick_manager::TickEvent;

pub(crate) type ResourceHistory<R> = HistoryBuffer<R>;

/// If there is a TickEvent and the client tick suddenly changes, we need
/// to update the ticks in the history buffer.
///
/// The history buffer ticks are only relevant relative to the current client tick.
/// (i.e. X ticks in the past compared to the current tick)
pub(crate) fn handle_tick_event_resource_history<R: Resource>(
    trigger: Trigger<TickEvent>,
    res: Option<ResMut<ResourceHistory<R>>>,
) {
    match *trigger.event() {
        TickEvent::TickSnap { old_tick, new_tick } => {
            if let Some(mut history) = res {
                history.update_ticks(new_tick - old_tick)
            }
        }
    }
}

/// This system handles changes and removals of resources
pub(crate) fn update_resource_history<R: Resource + Clone>(
    resource: Option<Res<R>>,
    mut history: ResMut<ResourceHistory<R>>,
    tick_manager: Res<TickManager>,
    rollback: Res<Rollback>,
) {
    // tick for which we will record the history (either the current client tick or the current rollback tick)
    let tick = tick_manager.tick_or_rollback_tick(rollback.as_ref());

    if let Some(resource) = resource {
        if resource.is_changed() {
            history.add_update(tick, resource.clone());
        }
    // resource does not exist, it might have been just removed
    } else {
        match history.peek() {
            Some((_, HistoryState::Removed)) => (),
            // if there is no latest item or the latest item isn't a removal then the resource just got removed.
            _ => history.add_remove(tick),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prelude::client::RollbackState;
    use crate::prelude::AppComponentExt;
    use crate::prelude::Tick;
    use crate::tests::stepper::BevyStepper;
    use bevy::ecs::system::RunSystemOnce;
    use tracing::info;

    #[derive(Resource, Clone, PartialEq, Debug)]
    struct TestResource(f32);

    /// Test that the history gets updated correctly
    /// 1. Updating the TestResource resource
    /// 2. Removing the TestResource resource
    /// 3. Updating the TestResource resource during rollback
    /// 4. Removing the TestResource resource during rollback
    #[test]
    fn test_update_history() {
        let mut stepper = BevyStepper::default();
        stepper.client_app.add_resource_rollback::<TestResource>();

        // 1. Updating TestResource resource
        stepper
            .client_app
            .world_mut()
            .insert_resource(TestResource(1.0));
        stepper.frame_step();
        stepper
            .client_app
            .world_mut()
            .resource_mut::<TestResource>()
            .0 = 2.0;
        stepper.frame_step();
        let tick = stepper.client_tick();
        assert_eq!(
            stepper
                .client_app
                .world_mut()
                .get_resource_mut::<ResourceHistory<TestResource>>()
                .expect("Expected resource history to be added")
                .pop_until_tick(tick),
            Some(HistoryState::Updated(TestResource(2.0))),
            "Expected resource value to be updated in resource history"
        );

        // 2. Removing TestResource
        stepper
            .client_app
            .world_mut()
            .remove_resource::<TestResource>();
        stepper.frame_step();
        let tick = stepper.client_tick();
        assert_eq!(
            stepper
                .client_app
                .world_mut()
                .get_resource_mut::<ResourceHistory<TestResource>>()
                .expect("Expected resource history to be added")
                .pop_until_tick(tick),
            Some(HistoryState::Removed),
            "Expected resource value to be removed in resource history"
        );

        // 3. Updating TestResource during rollback
        let rollback_tick = Tick(10);
        stepper
            .client_app
            .world_mut()
            .insert_resource(Rollback::new(RollbackState::ShouldRollback {
                current_tick: rollback_tick,
            }));
        stepper
            .client_app
            .world_mut()
            .insert_resource(TestResource(3.0));
        let _ = stepper
            .client_app
            .world_mut()
            .run_system_once(update_resource_history::<TestResource>);
        assert_eq!(
            stepper
                .client_app
                .world_mut()
                .get_resource_mut::<ResourceHistory<TestResource>>()
                .expect("Expected resource history to be added")
                .pop_until_tick(rollback_tick),
            Some(HistoryState::Updated(TestResource(3.0))),
            "Expected resource value to be updated in resource history"
        );

        // 4. Removing TestResource during rollback
        stepper
            .client_app
            .world_mut()
            .remove_resource::<TestResource>();
        let _ = stepper
            .client_app
            .world_mut()
            .run_system_once(update_resource_history::<TestResource>);
        assert_eq!(
            stepper
                .client_app
                .world_mut()
                .get_resource_mut::<ResourceHistory<TestResource>>()
                .expect("Expected resource history to be added")
                .pop_until_tick(rollback_tick),
            Some(HistoryState::Removed),
            "Expected resource value to be removed from resource history"
        );
    }

    /// Test that the initial resource rollback works correctly even with client sync.
    /// Case:
    /// - spawn R on the client
    /// - client sync
    /// - rollback is triggered
    ///
    /// Check that the resource is NOT removed, because it existed before the sync.
    ///
    /// This is a regression test for a bug where the resource was removed during rollback.
    /// Since prediction plugins only run after `is_sync`, the resource wasn't inserted in the history buffer
    /// and was removed during rollback.
    #[test]
    fn test_initial_rollback() {
        // tracing_subscriber::FmtSubscriber::builder()
        //     .with_max_level(tracing::Level::DEBUG)
        //     .init();
        let mut stepper = BevyStepper::default_no_init();
        stepper.client_app.add_resource_rollback::<TestResource>();
        stepper.build();
        stepper.wait_for_connection();

        // insert resource before sync
        stepper
            .client_app
            .world_mut()
            .insert_resource(TestResource(1.0));
        stepper.frame_step();
        info!(
            "Just added: {:?}",
            stepper
                .client_app
                .world()
                .resource::<ResourceHistory<TestResource>>()
        );
        let client_tick = stepper.client_tick();

        // sync
        stepper.wait_for_sync();
        info!(
            "{:?}",
            stepper
                .client_app
                .world()
                .resource::<ResourceHistory<TestResource>>()
        );

        // Initiate rollback
        let rollback_tick = stepper.server_tick() + 1;
        stepper
            .client_app
            .world_mut()
            .insert_resource(Rollback::new(RollbackState::ShouldRollback {
                current_tick: rollback_tick,
            }));
        stepper.frame_step();

        // Check that the resource still exists
        assert!(stepper
            .client_app
            .world_mut()
            .get_resource::<TestResource>()
            .is_some());
    }
}
