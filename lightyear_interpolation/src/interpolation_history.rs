use crate::prelude::InterpolationRegistry;
use bevy_ecs::prelude::*;
use bevy_reflect::Reflect;
use bevy_replicon::client::confirm_history::ConfirmHistory;
use bevy_utils::prelude::DebugName;
use lightyear_core::history_buffer::{HistoryBuffer, HistoryState};
use lightyear_core::interpolation::Interpolated;
use lightyear_core::prelude::Tick;
use lightyear_replication::checkpoint::ReplicationCheckpointMap;
#[allow(unused_imports)]
use tracing::{info, trace};

/// Stores a buffer of past component values received from the remote
#[derive(Component, Debug, Reflect)]
pub struct ConfirmedHistory<C> {
    history: HistoryBuffer<C>,
}

impl<C> Default for ConfirmedHistory<C> {
    fn default() -> Self {
        Self {
            history: HistoryBuffer::<C>::default(),
        }
    }
}

impl<C> PartialEq for ConfirmedHistory<C> {
    fn eq(&self, other: &Self) -> bool {
        self.history.eq(&other.history)
    }
}

impl<C> ConfirmedHistory<C> {
    pub(crate) fn len(&self) -> usize {
        self.history.len()
    }

    /// Get the n-th oldest tick in the buffer (starts from n = 0)
    pub fn get_nth_tick(&self, n: usize) -> Option<Tick> {
        self.history.get_nth(n).map(|(t, _)| *t)
    }

    /// The oldest value in the history, which is used as the start value for the interpolation
    pub fn start(&self) -> Option<(Tick, &C)> {
        self.get_nth(0)
    }

    /// The second oldest value in the history, which is used as the end value for the interpolation
    pub fn end(&self) -> Option<(Tick, &C)> {
        self.get_nth(1)
    }

    /// The most recent value in the history.
    pub fn newest(&self) -> Option<(Tick, &C)> {
        match self.history.most_recent() {
            None | Some((_, HistoryState::Removed)) => None,
            Some((t, HistoryState::Updated(v))) => Some((*t, v)),
        }
    }

    /// Get the n-th oldest tick in the buffer (starts from n = 0)
    pub(crate) fn get_nth(&self, n: usize) -> Option<(Tick, &C)> {
        match self.history.get_nth(n) {
            None | Some((_, HistoryState::Removed)) => None,
            Some((t, HistoryState::Updated(v))) => Some((*t, v)),
        }
    }

    /// Push a new value in the history.
    /// It MUST be more recent than all previous values, which is guaranteed from
    /// how lightyear_replication::receive works
    pub fn push(&mut self, tick: Tick, value: C) {
        self.history.add_update(tick, value)
    }

    /// Pop the oldest value in the history
    pub fn pop(&mut self) -> Option<(Tick, C)> {
        match self.history.pop() {
            None | Some((_, HistoryState::Removed)) => None,
            Some((t, HistoryState::Updated(v))) => Some((t, v)),
        }
    }
}

impl<C: Component + Clone> ConfirmedHistory<C> {
    pub fn interpolate(
        &self,
        interpolation_tick: Tick,
        interpolation_overstep: f32,
        interpolation_registry: &InterpolationRegistry,
    ) -> Option<C> {
        let (start_tick, start) = self.start()?;
        // It is possible that the interpolation tick lags behind the buffered
        // anchors, for example if two fresh updates arrive after a long gap:
        // X...H1...H2. In that case interpolation should not run yet.
        if interpolation_tick < start_tick {
            return None;
        }

        let (end_tick, end) = self.end()?;
        // Clamp rather than extrapolate beyond the newest confirmed value. This
        // makes late packets converge to the freshest server state instead of
        // overshooting when motion changes direction.
        let fraction = (((interpolation_tick - start_tick) as f32 + interpolation_overstep)
            / (end_tick - start_tick) as f32)
            .clamp(0.0, 1.0);
        trace!(
            ?start_tick,
            ?end_tick,
            ?interpolation_tick,
            ?interpolation_overstep,
            ?fraction,
            "Interpolate {:?}",
            DebugName::type_name::<C>()
        );
        Some(interpolation_registry.interpolate(start.clone(), end.clone(), fraction))
    }
}

/// When `Interpolated` is added after component `C` was already replicated onto the entity,
/// seed `ConfirmedHistory<C>` from the current value so interpolation has an anchor immediately.
///
/// This is the branch-local equivalent of `main`'s `#1421` fix, adapted to the current
/// Replicon marker-fn receive path. Component updates for interpolated entities are normally
/// captured by `registry::write_history::<C>`, but that only runs on future network updates.
/// If `Interpolated` arrives after `C`, we need to synthesize the initial history entry from the
/// existing component value and the entity's latest confirmed Replicon tick.
pub(crate) fn insert_confirmed_history_on_interpolated<C: Component + Clone>(
    trigger: On<Add, Interpolated>,
    mut commands: Commands,
    checkpoints: Res<ReplicationCheckpointMap>,
    query: Query<(&C, &ConfirmHistory), Without<ConfirmedHistory<C>>>,
) {
    let Ok((component, confirm_history)) = query.get(trigger.entity) else {
        return;
    };

    let Some(tick) = checkpoints.get(confirm_history.last_tick()) else {
        debug_assert!(
            false,
            "missing authoritative checkpoint mapping while backfilling ConfirmedHistory"
        );
        return;
    };

    let mut history = ConfirmedHistory::<C>::default();
    history.push(tick, component.clone());
    commands.entity(trigger.entity).insert(history);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::InterpolationRegistry;
    use bevy_app::App;
    use bevy_ecs::component::Component;
    use bevy_replicon::prelude::RepliconTick;

    #[derive(Component, Clone, Debug, PartialEq)]
    struct TestComp(f32);

    fn lerp(start: TestComp, end: TestComp, t: f32) -> TestComp {
        TestComp(start.0 + (end.0 - start.0) * t)
    }

    fn registry() -> InterpolationRegistry {
        let mut registry = InterpolationRegistry::default();
        registry.set_interpolation::<TestComp>(lerp);
        registry
    }

    #[test]
    fn interpolate_clamps_to_newest_value_when_tick_is_past_end() {
        let mut history = ConfirmedHistory::<TestComp>::default();
        history.push(Tick(10), TestComp(0.0));
        history.push(Tick(20), TestComp(10.0));

        let registry = registry();
        assert_eq!(
            history.interpolate(Tick(30), 0.0, &registry),
            Some(TestComp(10.0))
        );
        assert_eq!(
            history.interpolate(Tick(20), 0.5, &registry),
            Some(TestComp(10.0))
        );
    }

    #[test]
    fn interpolate_returns_none_with_single_keyframe() {
        let mut history = ConfirmedHistory::<TestComp>::default();
        history.push(Tick(10), TestComp(42.0));

        let registry = registry();
        assert_eq!(history.interpolate(Tick(10), 0.0, &registry), None);
        assert_eq!(history.interpolate(Tick(50), 0.5, &registry), None);
    }

    #[test]
    fn inserts_history_when_interpolated_added_after_component_is_already_replicated() {
        let mut app = App::new();
        app.insert_resource(ReplicationCheckpointMap::default());
        app.add_observer(insert_confirmed_history_on_interpolated::<TestComp>);

        let replicon_tick = RepliconTick::new(11);
        app.world_mut()
            .resource_mut::<ReplicationCheckpointMap>()
            .record(replicon_tick, Tick(42));

        let entity = app
            .world_mut()
            .spawn((TestComp(2.0), ConfirmHistory::new(replicon_tick)))
            .id();
        app.update();
        app.world_mut().entity_mut(entity).insert(Interpolated);
        app.update();

        let history = app
            .world()
            .entity(entity)
            .get::<ConfirmedHistory<TestComp>>()
            .unwrap();
        assert_eq!(
            history.start().map(|(tick, value)| (tick, value.clone())),
            Some((Tick(42), TestComp(2.0)))
        );
    }
}
