use crate::prelude::InterpolationRegistry;
use bevy_ecs::prelude::*;
use bevy_reflect::Reflect;
use bevy_utils::prelude::DebugName;
use lightyear_core::history_buffer::{HistoryBuffer, HistoryState};
use lightyear_core::prelude::Tick;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::InterpolationRegistry;
    use bevy_ecs::component::Component;

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
}
