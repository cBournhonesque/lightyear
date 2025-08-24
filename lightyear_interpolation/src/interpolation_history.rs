use core::ops::Deref;

use crate::manager::InterpolationManager;
use crate::{Interpolated, SyncComponent};
use bevy_ecs::{
    change_detection::DetectChanges,
    component::Component,
    entity::Entity,
    query::{With, Without},
    system::{Commands, Query, Res, Single},
    world::Ref,
};
use lightyear_core::prelude::{LocalTimeline, Tick};
use lightyear_replication::components::Confirmed;
use lightyear_replication::registry::registry::ComponentRegistry;
use lightyear_utils::ready_buffer::ReadyBuffer;
use tracing::trace;

/// To know if we need to do rollback, we need to compare the interpolated entity's history with the server's state updates
#[derive(Component, Debug)]
pub struct ConfirmedHistory<C: Component> {
    // TODO: here we can use a sequence buffer. We won't store more than a couple

    // TODO: add a max size for the buffer
    // We want to avoid using a SequenceBuffer for optimization (we don't want to store a copy of the component for each history tick)
    // We can afford to use a ReadyBuffer because we will get server updates with monotonically increasing ticks
    // therefore we can get rid of the old ticks before the server update

    // We will only store the history for the ticks where the component got updated
    pub buffer: ReadyBuffer<Tick, C>,
}

impl<C: SyncComponent> Default for ConfirmedHistory<C> {
    fn default() -> Self {
        Self::new()
    }
}

// mostly used for tests
impl<C: SyncComponent> PartialEq for ConfirmedHistory<C> {
    fn eq(&self, other: &Self) -> bool {
        self.buffer.heap.iter().eq(other.buffer.heap.iter())
    }
}

impl<C: Component> ConfirmedHistory<C> {
    pub fn new() -> Self {
        Self {
            buffer: ReadyBuffer::new(),
        }
    }

    /// Reset the history for this component
    pub(crate) fn clear(&mut self) {
        self.buffer = ReadyBuffer::new();
    }

    pub(crate) fn peek(&mut self) -> Option<(Tick, &C)> {
        self.buffer.heap.peek().map(|item| (item.key, &item.item))
    }

    pub(crate) fn pop(&mut self) -> Option<(Tick, C)> {
        self.buffer.heap.pop().map(|item| (item.key, item.item))
    }

    /// Get the value of the component at the specified tick.
    /// Clears the history buffer of all ticks older or equal than the specified tick.
    /// NOTE: doesn't pop the last value!
    /// CAREFUL:
    /// the component history will only contain the ticks where the component got updated, and otherwise
    /// contains gaps. Therefore, we need to always leave a value in the history buffer so that we can
    /// get the values for the future ticks
    pub(crate) fn pop_until_tick(&mut self, tick: Tick) -> Option<(Tick, C)> {
        self.buffer.pop_until(&tick)
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
            && !confirmed_component.is_added()
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
            history.buffer.push(tick, component);

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
