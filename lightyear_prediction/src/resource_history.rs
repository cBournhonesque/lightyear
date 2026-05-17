//! There's a lot of overlap with `client::prediction_history` because resources are components in ECS so rollback is going to look similar.
use crate::manager::PredictionManager;
use bevy_ecs::prelude::*;
use bevy_utils::prelude::DebugName;
use lightyear_core::history_buffer::{HistoryBuffer, HistoryState};
use lightyear_core::prelude::LocalTimeline;
use lightyear_core::timeline::SyncEvent;
use lightyear_sync::prelude::client::InputTimelineConfig;
#[allow(unused_imports)]
use tracing::{info, trace};

pub(crate) type ResourceHistory<R> = HistoryBuffer<R>;

/// If there is a TickEvent and the client tick suddenly changes, we need
/// to update the ticks in the history buffer.
///
/// The history buffer ticks are only relevant relative to the current client tick.
/// (i.e. X ticks in the past compared to the current tick)
pub(crate) fn handle_tick_event_resource_history<R: Resource>(
    trigger: On<SyncEvent<InputTimelineConfig>>,
    res: Option<ResMut<ResourceHistory<R>>>,
) {
    if let Some(mut history) = res {
        history.update_ticks(trigger.tick_delta)
    }
}

/// Make sure that pre-existing resources get populated in the ResourceHistory
/// as soon as PredictionManager is added
pub(crate) fn update_resource_history_on_prediction_manager_added<R: Resource + Clone>(
    _: On<Add, PredictionManager>,
    timeline: Res<LocalTimeline>,
    mut history: ResMut<ResourceHistory<R>>,
    resource: Option<Res<R>>,
) {
    let tick = timeline.tick();
    if let Some(resource) = resource {
        history.add_update(tick, resource.clone());
    }
}

/// This system handles changes and removals of resources
pub(crate) fn update_resource_history<R: Resource + Clone>(
    resource: Option<Res<R>>,
    mut history: ResMut<ResourceHistory<R>>,
    timeline: Res<LocalTimeline>,
) {
    // tick for which we will record the history (either the current client tick or the current rollback tick)
    let tick = timeline.tick();
    let kind = DebugName::type_name::<R>();

    if let Some(resource) = resource {
        if resource.is_changed() {
            trace!(?tick, ?kind, "Adding resource to history");
            history.add_update(tick, resource.clone());
        }
    // resource does not exist, it might have been just removed
    } else {
        match history.peek() {
            Some((_, HistoryState::Removed)) => (),
            // if there is no latest item or the latest item isn't a removal then the resource just got removed.
            _ => {
                trace!(?tick, ?kind, "Adding resource removal to history");
                history.add_remove(tick)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rollback::prepare_rollback_resource;
    use bevy_ecs::system::RunSystemOnce;
    use lightyear_core::prelude::{Rollback, Tick};

    #[derive(Resource, Clone, PartialEq, Debug)]
    struct TestResource(f32);

    /// Test that initial resource rollback does not remove a resource when
    /// the rollback tick predates the first history entry.
    #[test]
    fn test_initial_rollback() {
        let rollback_tick = Tick(10);
        let mut world = World::new();
        world.insert_resource(ResourceHistory::<TestResource>::default());
        world.insert_resource(TestResource(1.0));

        let manager = world
            .spawn((PredictionManager::default(), Rollback::FromState))
            .id();
        world
            .get_mut::<PredictionManager>(manager)
            .unwrap()
            .set_rollback_tick(rollback_tick);

        world
            .run_system_once(prepare_rollback_resource::<TestResource>)
            .unwrap();

        assert_eq!(
            world.get_resource::<TestResource>(),
            Some(&TestResource(1.0))
        );
    }
}
