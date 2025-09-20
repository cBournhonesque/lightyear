use crate::manager::InterpolationManager;
use crate::prelude::InterpolationRegistry;
use crate::{Interpolated, SyncComponent};
use bevy_ecs::{
    change_detection::DetectChanges,
    component::Component,
    entity::Entity,
    query::{With, Without},
    system::{Commands, Query, Res, Single},
    world::Ref,
};
use bevy_reflect::Reflect;
use core::ops::Deref;
use lightyear_core::history_buffer::{HistoryBuffer, HistoryState};
use lightyear_core::prelude::{LocalTimeline, Tick};
use lightyear_replication::components::Confirmed;
use lightyear_replication::registry::registry::ComponentRegistry;
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
                core::any::type_name::<C>()
            );
            return Some(interpolation_registry.interpolate(start.clone(), end.clone(), fraction));
        }
        None
    }
}

/// When we receive a server update for an interpolated component, we need to store it in the confirmed history,
pub(crate) fn apply_confirmed_update_mode_full<C: SyncComponent>(
    component_registry: Res<ComponentRegistry>,
    // TODO: use the interpolation receiver corresponding to the Confirmed entity (via Replicated)
    query: Single<(&LocalTimeline, &InterpolationManager)>,
    mut interpolated_entities: Query<
        &mut ConfirmedHistory<C>,
        (With<Interpolated>, Without<Confirmed>),
    >,
    confirmed_entities: Query<(Entity, &Confirmed, Ref<C>)>,
) {
    let kind = core::any::type_name::<C>();
    let (timeline, manager) = query.into_inner();
    for (confirmed_entity, confirmed, confirmed_component) in confirmed_entities.iter() {
        if let Some(p) = confirmed.interpolated
            && confirmed_component.is_changed()
            && let Ok(mut history) = interpolated_entities.get_mut(p)
        {
            // // if has_authority is true, we will consider the Confirmed value as the source of truth
            // // else it will be the server updates
            // // TODO: as an alternative, we could set the confirmed.tick to be equal to the current tick
            // //  if we have authority! Then it would also work for prediction?
            // let tick = if has_authority {
            //     timeline.tick()
            // } else {
            //     confirmed.tick
            // };
            let tick = confirmed.tick;

            // let Some(tick) = client
            //     .replication_receiver()
            //     .get_confirmed_tick(confirmed_entity)
            // else {
            //     error!(
            //         "Could not find replication channel for entity {:?}",
            //         confirmed_entity
            //     );
            //     continue;
            // };

            // map any entities from confirmed to predicted
            let mut component = confirmed_component.deref().clone();
            let _ = manager.map_entities(&mut component, component_registry.as_ref());
            trace!(?kind, tick = ?tick, "adding confirmed update to history");
            // update the history at the value that the entity currently is
            // NOTE: it is guaranteed that the confirmed update is more recent than all previous updates
            //  We enforce this invariant in replication::receive
            history.push(tick, component);

            // TODO: here we do not want to update directly the component, that will be done during interpolation
        }
    }
}

/// When we receive a server update for a simple component, we just update the entity directly
pub(crate) fn apply_confirmed_update_mode_simple<C: SyncComponent>(
    component_registry: Res<ComponentRegistry>,
    // TODO: handle multiple interpolation receivers
    manager: Single<&InterpolationManager>,
    mut interpolated_entities: Query<&mut C, (With<Interpolated>, Without<Confirmed>)>,
    confirmed_entities: Query<(&Confirmed, Ref<C>)>,
) {
    for (confirmed, confirmed_component) in confirmed_entities.iter() {
        if let Some(p) = confirmed.interpolated
            && confirmed_component.is_changed()
            && !confirmed_component.is_added()
            && let Ok(mut interpolated_component) = interpolated_entities.get_mut(p)
        {
            // for sync-components, we just match the confirmed component
            // map any entities from confirmed to interpolated first
            let mut component = confirmed_component.deref().clone();
            let _ = manager.map_entities(&mut component, component_registry.as_ref());
            *interpolated_component = component;
        }
    }
}

/// When we receive a server update for a simple component, we just update the entity directly
pub(crate) fn apply_confirmed_update_immutable_mode_simple<C: Component + Clone>(
    component_registry: Res<ComponentRegistry>,
    // TODO: handle multiple interpolation receivers
    manager: Single<&InterpolationManager>,
    mut interpolated_entities: Query<(), (With<Interpolated>, Without<Confirmed>)>,
    confirmed_entities: Query<(&Confirmed, Ref<C>)>,
    mut commands: Commands,
) {
    for (confirmed, confirmed_component) in confirmed_entities.iter() {
        if let Some(p) = confirmed.interpolated
            && confirmed_component.is_changed()
            && !confirmed_component.is_added()
            && let Ok(()) = interpolated_entities.get_mut(p)
        {
            // for sync-components, we just match the confirmed component
            // map any entities from confirmed to interpolated first
            let mut component = confirmed_component.deref().clone();
            let _ = manager.map_entities(&mut component, component_registry.as_ref());
            commands.entity(p).try_insert(component);
        }
    }
}
