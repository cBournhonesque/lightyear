//! There's a lot of overlap with `client::prediction_history` because resources are components in ECS so rollback is going to look similar.
use bevy::prelude::*;

use crate::{
    prelude::{Tick, TickManager},
    utils::ready_buffer::ReadyBuffer,
};

use super::rollback::Rollback;

/// Stores a past update for a resource
#[derive(Debug, PartialEq, Clone)]
pub(crate) enum ResourceState<R> {
    /// the resource just got removed
    Removed,
    /// the resource got updated
    Updated(R),
}

/// To know if we need to do rollback, we need to compare the resource's history with the server's state updates
#[derive(Resource, Debug)]
pub(crate) struct ResourceHistory<R> {
    // We will only store the history for the ticks where the resource got updated
    pub buffer: ReadyBuffer<Tick, ResourceState<R>>,
}

impl<R> Default for ResourceHistory<R> {
    fn default() -> Self {
        Self {
            buffer: ReadyBuffer::new(),
        }
    }
}

impl<R> PartialEq for ResourceHistory<R> {
    fn eq(&self, other: &Self) -> bool {
        let mut self_history: Vec<_> = self.buffer.heap.iter().collect();
        let mut other_history: Vec<_> = other.buffer.heap.iter().collect();
        self_history.sort_by_key(|item| item.key);
        other_history.sort_by_key(|item| item.key);
        self_history.eq(&other_history)
    }
}

impl<R: Clone> ResourceHistory<R> {
    /// Reset the history for this resource
    pub(crate) fn clear(&mut self) {
        self.buffer = ReadyBuffer::new();
    }

    /// Add to the buffer that we received an update for the resource at the given tick
    pub(crate) fn add_update(&mut self, tick: Tick, resource: R) {
        self.buffer.push(tick, ResourceState::Updated(resource));
    }

    /// Add to the buffer that the resource got removed at the given tick
    pub(crate) fn add_remove(&mut self, tick: Tick) {
        self.buffer.push(tick, ResourceState::Removed);
    }

    // TODO: check if this logic is necessary/correct?
    /// Clear the history of values strictly older than the specified tick,
    /// and return the most recent value that is older or equal to the specified tick.
    /// NOTE: That value is written back into the buffer
    ///
    /// CAREFUL:
    /// the resource history will only contain the ticks where the resource got updated, and otherwise
    /// contains gaps. Therefore, we need to always leave a value in the history buffer so that we can
    /// get the values for the future ticks
    pub(crate) fn pop_until_tick(&mut self, tick: Tick) -> Option<ResourceState<R>> {
        self.buffer.pop_until(&tick).map(|(tick, state)| {
            // TODO: this clone is pretty bad and avoidable. Probably switch to a sequence buffer?
            self.buffer.push(tick, state.clone());
            state
        })
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
        match history.buffer.peek_max_item() {
            Some((_, ResourceState::Removed)) => (),
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
    use crate::tests::stepper::BevyStepper;
    use crate::utils::ready_buffer::ItemWithReadyKey;
    use bevy::ecs::system::RunSystemOnce;

    #[derive(Resource, Clone, PartialEq, Debug)]
    struct TestResource(f32);

    /// Test adding and removing updates to the resource history
    #[test]
    fn test_resource_history() {
        let mut resource_history = ResourceHistory::<TestResource>::default();

        // check when we try to access a value when the buffer is empty
        assert_eq!(resource_history.pop_until_tick(Tick(0)), None);

        // check when we try to access an exact tick
        resource_history.add_update(Tick(1), TestResource(1.0));
        resource_history.add_update(Tick(2), TestResource(2.0));
        assert_eq!(
            resource_history.pop_until_tick(Tick(2)),
            Some(ResourceState::Updated(TestResource(2.0)))
        );
        // check that we cleared older ticks, and that the most recent value still remains
        assert_eq!(resource_history.buffer.len(), 1);
        assert!(resource_history.buffer.has_item(&Tick(2)));

        // check when we try to access a value in-between ticks
        resource_history.add_update(Tick(4), TestResource(4.0));
        // we retrieve the most recent value older or equal to Tick(3)
        assert_eq!(
            resource_history.pop_until_tick(Tick(3)),
            Some(ResourceState::Updated(TestResource(2.0)))
        );
        assert_eq!(resource_history.buffer.len(), 2);
        // check that the most recent value got added back to the buffer at the popped tick
        assert_eq!(
            resource_history.buffer.heap.peek(),
            Some(&ItemWithReadyKey {
                key: Tick(2),
                item: ResourceState::Updated(TestResource(2.0))
            })
        );
        assert!(resource_history.buffer.has_item(&Tick(4)));

        // check that nothing happens when we try to access a value before any ticks
        assert_eq!(resource_history.pop_until_tick(Tick(0)), None);
        assert_eq!(resource_history.buffer.len(), 2);

        resource_history.add_remove(Tick(5));
        assert_eq!(resource_history.buffer.len(), 3);

        resource_history.clear();
        assert_eq!(resource_history.buffer.len(), 0);
    }

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
            Some(ResourceState::Updated(TestResource(2.0))),
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
            Some(ResourceState::Removed),
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
        stepper
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
            Some(ResourceState::Updated(TestResource(3.0))),
            "Expected resource value to be updated in resource history"
        );

        // 4. Removing TestResource during rollback
        stepper
            .client_app
            .world_mut()
            .remove_resource::<TestResource>();
        stepper
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
            Some(ResourceState::Removed),
            "Expected resource value to be removed from resource history"
        );
    }
}
