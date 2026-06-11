use crate::prelude::InterpolationRegistry;
use bevy_ecs::prelude::*;
use bevy_reflect::Reflect;
use bevy_replicon::client::confirm_history::ConfirmHistory;
use bevy_replicon::prelude::{Diffable as RepliconDiffable, PatchIndex};
use bevy_replicon::shared::replication::diff::DiffReceiver;
use bevy_utils::prelude::DebugName;
use lightyear_core::history_buffer::{HistoryBuffer, HistoryState};
use lightyear_core::interpolation::Interpolated;
use lightyear_core::prelude::Tick;
use lightyear_replication::checkpoint::ReplicationCheckpointMap;
#[allow(unused_imports)]
use tracing::{info, trace};

/// Stores a buffer of past component values received from the remote
#[derive(Component, Debug, Reflect)]
pub struct ConfirmedHistory<C, M = ()> {
    history: HistoryBuffer<C, M>,
    /// True when the newest anchor was synthesized from an empty mutate tick.
    ///
    /// Empty mutate ticks confirm that a component did not change. While this stays true, newer
    /// empty mutate ticks can slide the same anchor forward instead of cloning the value again.
    newest_is_unchanged: bool,
}

#[derive(Debug, PartialEq)]
pub(crate) enum ConfirmedHistorySample<C> {
    Pending,
    Removed,
    Present(C),
}

impl<C, M> Default for ConfirmedHistory<C, M> {
    fn default() -> Self {
        Self {
            history: HistoryBuffer::<C, M>::default(),
            newest_is_unchanged: false,
        }
    }
}

impl<C, M> PartialEq for ConfirmedHistory<C, M> {
    fn eq(&self, other: &Self) -> bool {
        self.history.eq(&other.history) && self.newest_is_unchanged == other.newest_is_unchanged
    }
}

impl<C, M> ConfirmedHistory<C, M> {
    pub(crate) fn len(&self) -> usize {
        self.history.len()
    }

    /// Get the n-th oldest tick in the buffer (starts from n = 0)
    pub fn get_nth_tick(&self, n: usize) -> Option<Tick> {
        self.history.get_nth(n).map(|(t, _)| *t)
    }

    fn get_nth_state(&self, n: usize) -> Option<(Tick, &HistoryState<C>)> {
        self.history.get_nth(n).map(|(t, state)| (*t, state))
    }

    /// The oldest value in the history.
    pub fn start(&self) -> Option<(Tick, &C)> {
        self.get_nth(0)
    }

    /// The second oldest value in the history.
    pub fn end(&self) -> Option<(Tick, &C)> {
        self.get_nth(1)
    }

    /// Returns the newest value at or before `interpolation_tick`, plus the next newer value.
    ///
    /// Diff-enabled histories can retain older patch-cursor bases after they are no longer useful
    /// for visual interpolation. Callers that need to interpolate manually should use these bounds
    /// instead of [`Self::start`] and [`Self::end`].
    pub fn interpolation_bounds(
        &self,
        interpolation_tick: Tick,
    ) -> Option<((Tick, &C), Option<(Tick, &C)>)> {
        let previous_index = (0..self.len())
            .take_while(|i| {
                self.get_nth_tick(*i)
                    .is_some_and(|tick| tick <= interpolation_tick)
            })
            .last()?;

        Some((
            self.get_nth(previous_index)?,
            self.get_nth(previous_index + 1),
        ))
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
    pub fn push_with_metadata(&mut self, tick: Tick, value: C, metadata: M) {
        self.history.add_with_metadata(tick, Some(value), metadata);
        self.newest_is_unchanged = false;
    }

    /// Push a removal in the history.
    pub(crate) fn push_remove_with_metadata(&mut self, tick: Tick, metadata: M) {
        self.history.add_with_metadata(tick, None, metadata);
        self.newest_is_unchanged = false;
    }

    pub fn value_with_metadata(&self, metadata: &M) -> Option<&C>
    where
        M: PartialEq,
    {
        self.history.get_with_metadata(metadata)
    }

    /// Pop the oldest value in the history
    pub fn pop(&mut self) -> Option<(Tick, C)> {
        let popped = match self.history.pop() {
            None | Some((_, HistoryState::Removed)) => None,
            Some((t, HistoryState::Updated(v))) => Some((t, v)),
        };
        if self.history.len() == 0 {
            self.newest_is_unchanged = false;
        }
        popped
    }
}

impl<C> ConfirmedHistory<C, Option<PatchIndex>> {
    pub(crate) fn prune_metadata_before_cursor(&mut self, min_cursor: PatchIndex) {
        self.history
            .retain(|_, _, metadata| retain_diff_metadata_anchor(metadata, min_cursor));
        if self.history.len() == 0 {
            self.newest_is_unchanged = false;
        }
    }
}

impl<C, M: Default> ConfirmedHistory<C, M> {
    /// Push a new value in the history.
    /// It MUST be more recent than all previous values, which is guaranteed from
    /// how lightyear_replication::receive works
    pub fn push(&mut self, tick: Tick, value: C) {
        self.push_with_metadata(tick, value, M::default());
    }

    /// Push a removal in the history.
    pub(crate) fn push_remove(&mut self, tick: Tick) {
        self.push_remove_with_metadata(tick, M::default());
    }
}

fn retain_diff_metadata_anchor(metadata: &Option<PatchIndex>, min_cursor: PatchIndex) -> bool {
    match metadata {
        Some(cursor) => *cursor >= min_cursor,
        None => min_cursor == 0,
    }
}

impl<C: Clone, M: Clone> ConfirmedHistory<C, M> {
    /// Mark the newest value as unchanged at `tick`.
    ///
    /// If the newest anchor was already synthesized from an empty mutate tick, only its tick is
    /// advanced. Otherwise a single unchanged anchor is appended by cloning the newest value.
    pub(crate) fn push_unchanged(&mut self, tick: Tick) -> Option<Tick> {
        let (newest_tick, newest_value, newest_metadata) =
            match self.history.most_recent_with_metadata() {
                Some((newest_tick, HistoryState::Updated(newest_value), newest_metadata)) => {
                    (newest_tick, newest_value.clone(), newest_metadata.clone())
                }
                None | Some((_, HistoryState::Removed, _)) => return None,
            };
        if tick <= newest_tick {
            return None;
        }

        if self.newest_is_unchanged {
            self.history.set_most_recent_tick(tick);
        } else {
            self.history
                .add_with_metadata(tick, Some(newest_value), newest_metadata);
            self.newest_is_unchanged = true;
        }
        Some(newest_tick)
    }
}

impl<C: Component + Clone, M> ConfirmedHistory<C, M> {
    pub(crate) fn sample(
        &self,
        interpolation_tick: Tick,
        interpolation_overstep: f32,
        interpolation_registry: &InterpolationRegistry,
    ) -> ConfirmedHistorySample<C> {
        let Some(previous_index) = (0..self.len())
            .take_while(|i| {
                self.get_nth_tick(*i)
                    .is_some_and(|tick| tick <= interpolation_tick)
            })
            .last()
        else {
            return ConfirmedHistorySample::Pending;
        };

        let Some((start_tick, start_state)) = self.get_nth_state(previous_index) else {
            return ConfirmedHistorySample::Pending;
        };
        let HistoryState::Updated(start) = start_state else {
            return ConfirmedHistorySample::Removed;
        };

        let Some((end_tick, HistoryState::Updated(end))) = self.get_nth_state(previous_index + 1)
        else {
            return ConfirmedHistorySample::Present(start.clone());
        };

        if !interpolation_registry.has_interpolation_fn::<C>() {
            return ConfirmedHistorySample::Present(start.clone());
        }

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
        trace!(
            target: "lightyear_debug::interpolation",
            kind = "interpolation_history_sample",
            component = ?DebugName::type_name::<C>(),
            interpolation_tick = interpolation_tick.0,
            start_tick = start_tick.0,
            end_tick = end_tick.0,
            interpolation_overstep,
            fraction,
            history_len = self.len(),
            "sampled interpolation history"
        );
        ConfirmedHistorySample::Present(interpolation_registry.interpolate(
            start.clone(),
            end.clone(),
            fraction,
        ))
    }

    pub fn interpolate(
        &self,
        interpolation_tick: Tick,
        interpolation_overstep: f32,
        interpolation_registry: &InterpolationRegistry,
    ) -> Option<C> {
        let ((start_tick, start), end) = self.interpolation_bounds(interpolation_tick)?;
        let Some((end_tick, end)) = end else {
            return (self.len() > 1).then(|| start.clone());
        };
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
        trace!(
            target: "lightyear_debug::interpolation",
            kind = "interpolation_history_sample",
            component = ?DebugName::type_name::<C>(),
            interpolation_tick = interpolation_tick.0,
            start_tick = start_tick.0,
            end_tick = end_tick.0,
            interpolation_overstep,
            fraction,
            history_len = self.len(),
            "sampled interpolation history"
        );
        Some(interpolation_registry.interpolate(start.clone(), end.clone(), fraction))
    }
}

/// When `Interpolated` and component `C` are both present on an entity, seed
/// `ConfirmedHistory<C>` from the current value so interpolation has an anchor immediately.
///
/// This is the branch-local equivalent of `main`'s `#1421` fix, adapted to the current
/// Replicon marker-fn receive path. Component updates for interpolated entities are normally
/// captured by `registry::write_history::<C>`, but that only runs on future network updates.
/// If the marker and component are not written in the same pass, we need to synthesize the initial
/// history entry from the existing component value and the entity's latest confirmed Replicon tick.
pub(crate) fn insert_confirmed_history_on_interpolated<C, M>(
    trigger: On<Add, (C, Interpolated)>,
    mut commands: Commands,
    checkpoints: Res<ReplicationCheckpointMap>,
    query: Query<(&C, &ConfirmHistory), (With<Interpolated>, Without<ConfirmedHistory<C, M>>)>,
) where
    C: Component + Clone,
    M: Default + Send + Sync + 'static,
{
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

    let mut history = ConfirmedHistory::<C, M>::default();
    history.push(tick, component.clone());
    commands
        .entity(trigger.entity)
        .try_insert(history)
        .try_remove::<C>();
}

pub(crate) fn insert_confirmed_history_on_interpolated_diff<C>(
    trigger: On<Add, (C, Interpolated)>,
    mut commands: Commands,
    checkpoints: Res<ReplicationCheckpointMap>,
    query: Query<
        (&C, &ConfirmHistory, Option<&DiffReceiver<C>>),
        (
            With<Interpolated>,
            Without<ConfirmedHistory<C, Option<PatchIndex>>>,
        ),
    >,
) where
    C: Component + Clone + RepliconDiffable,
{
    let Ok((component, confirm_history, receiver)) = query.get(trigger.entity) else {
        return;
    };

    let Some(tick) = checkpoints.get(confirm_history.last_tick()) else {
        debug_assert!(
            false,
            "missing authoritative checkpoint mapping while backfilling diff ConfirmedHistory"
        );
        return;
    };

    let cursor = receiver.map(DiffReceiver::last_applied).unwrap_or(None);
    let component = component.clone();

    let mut history = ConfirmedHistory::<C, Option<PatchIndex>>::default();
    history.push_with_metadata(tick, component, cursor);
    commands
        .entity(trigger.entity)
        .try_insert(history)
        .try_remove::<C>();
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::InterpolationRegistry;
    use bevy_app::App;
    use bevy_ecs::component::Component;
    use bevy_replicon::prelude::{Diffable, RepliconTick};
    use serde::{Deserialize, Serialize};

    #[derive(Component, Clone, Debug, PartialEq)]
    struct TestComp(f32);

    #[derive(Component, Clone, Debug, PartialEq, Serialize, Deserialize)]
    struct TestDiffComp(u32);

    impl Diffable for TestDiffComp {
        type Patch = u32;

        fn apply_patch(&mut self, patch: &Self::Patch) -> bevy_ecs::error::Result<()> {
            self.0 = *patch;
            Ok(())
        }
    }

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
    fn interpolation_bounds_skip_retained_diff_bases() {
        let mut history = ConfirmedHistory::<TestComp, Option<PatchIndex>>::default();
        for i in 0..=12 {
            history.push_with_metadata(Tick(i), TestComp(i as f32), Some(u64::from(i)));
        }

        let ((start_tick, start), end) = history.interpolation_bounds(Tick(10)).unwrap();

        assert_eq!(start_tick, Tick(10));
        assert_eq!(*start, TestComp(10.0));
        assert_eq!(
            end.map(|(tick, value)| (tick, value.clone())),
            Some((Tick(11), TestComp(11.0)))
        );
    }

    #[test]
    fn interpolate_uses_bounds_when_history_retains_old_bases() {
        let mut history = ConfirmedHistory::<TestComp, Option<PatchIndex>>::default();
        for i in 0..=12 {
            history.push_with_metadata(Tick(i), TestComp(i as f32), Some(u64::from(i)));
        }

        let registry = registry();

        assert_eq!(
            history.interpolate(Tick(10), 0.5, &registry),
            Some(TestComp(10.5))
        );
    }

    #[test]
    fn inserts_history_when_interpolated_added_after_component_is_already_replicated() {
        let mut app = App::new();
        app.insert_resource(ReplicationCheckpointMap::default());
        app.add_observer(insert_confirmed_history_on_interpolated::<TestComp, ()>);

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
        assert!(
            !app.world().entity(entity).contains::<TestComp>(),
            "live interpolated component should be removed until the interpolation timeline reaches the history start tick"
        );
    }

    #[test]
    fn inserts_history_when_component_added_after_interpolated_marker() {
        let mut app = App::new();
        app.insert_resource(ReplicationCheckpointMap::default());
        app.add_observer(insert_confirmed_history_on_interpolated::<TestComp, ()>);

        let replicon_tick = RepliconTick::new(11);
        app.world_mut()
            .resource_mut::<ReplicationCheckpointMap>()
            .record(replicon_tick, Tick(42));

        let entity = app
            .world_mut()
            .spawn((Interpolated, ConfirmHistory::new(replicon_tick)))
            .id();
        app.update();
        app.world_mut().entity_mut(entity).insert(TestComp(2.0));
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
        assert!(
            !app.world().entity(entity).contains::<TestComp>(),
            "live interpolated component should be removed until the interpolation timeline reaches the history start tick"
        );
    }

    #[test]
    fn inserts_diff_history_with_receiver_cursor_when_component_added_after_marker() {
        let mut app = App::new();
        app.insert_resource(ReplicationCheckpointMap::default());
        app.add_observer(insert_confirmed_history_on_interpolated_diff::<TestDiffComp>);

        let replicon_tick = RepliconTick::new(11);
        app.world_mut()
            .resource_mut::<ReplicationCheckpointMap>()
            .record(replicon_tick, Tick(42));

        let entity = app
            .world_mut()
            .spawn((
                Interpolated,
                ConfirmHistory::new(replicon_tick),
                DiffReceiver::<TestDiffComp>::new(Some(7)),
            ))
            .id();
        app.update();
        app.world_mut().entity_mut(entity).insert(TestDiffComp(2));
        app.update();

        let history = app
            .world()
            .entity(entity)
            .get::<ConfirmedHistory<TestDiffComp, Option<PatchIndex>>>()
            .unwrap();
        assert_eq!(
            history.start().map(|(tick, value)| (tick, value.clone())),
            Some((Tick(42), TestDiffComp(2)))
        );
        assert_eq!(
            history.value_with_metadata(&Some(7)).cloned(),
            Some(TestDiffComp(2))
        );
        assert!(
            !app.world().entity(entity).contains::<TestDiffComp>(),
            "live interpolated component should be removed until the interpolation timeline reaches the history start tick"
        );
    }
}
