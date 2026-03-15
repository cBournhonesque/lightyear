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
        if let Some((start_tick, start)) = self.start()
            && let Some((end_tick, end)) = self.end()
        {
            if interpolation_tick < start_tick {
                return None;
            }
            let fraction = ((interpolation_tick - start_tick) as f32 + interpolation_overstep)
                / (end_tick - start_tick) as f32;
            trace!(
                ?start_tick,
                ?end_tick,
                ?interpolation_tick,
                ?interpolation_overstep,
                ?fraction,
                "Interpolate {:?}",
                DebugName::type_name::<C>()
            );
            return Some(interpolation_registry.interpolate(start.clone(), end.clone(), fraction));
        }
        None
    }
}