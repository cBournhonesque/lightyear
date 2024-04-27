use std::ops::Deref;

use crate::_internal::FromType;
use bevy::prelude::{
    Commands, Component, DetectChanges, Entity, Query, Ref, Res, ResMut, With, Without,
};
use tracing::{debug, trace};

use crate::client::components::{ComponentSyncMode, SyncComponent};
use crate::client::components::{Confirmed, SyncMetadata};
use crate::client::connection::ConnectionManager;
use crate::client::interpolation::interpolate::InterpolateStatus;
use crate::client::interpolation::resource::InterpolationManager;
use crate::client::interpolation::Interpolated;
use crate::prelude::{ComponentRegistry, ExternalMapper, TickManager};

use crate::shared::tick_manager::Tick;
use crate::utils::ready_buffer::ReadyBuffer;

/// To know if we need to do rollback, we need to compare the interpolated entity's history with the server's state updates
#[derive(Component, Debug)]
pub struct ConfirmedHistory<C: SyncComponent> {
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

impl<C: SyncComponent> ConfirmedHistory<C> {
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

// TODO: maybe add the component history on the Confirmed entity instead of Interpolated? would make more sense maybe
pub(crate) fn add_component_history<C: SyncComponent>(
    component_registry: Res<ComponentRegistry>,
    // TODO: unfortunately we need this to be mutable because of the MapEntities trait even though it's not actually needed...
    mut manager: ResMut<InterpolationManager>,
    tick_manager: Res<TickManager>,
    mut commands: Commands,
    connection: Res<ConnectionManager>,
    interpolated_entities: Query<Entity, (Without<ConfirmedHistory<C>>, With<Interpolated>)>,
    confirmed_entities: Query<(&Confirmed, Ref<C>)>,
) {
    let current_tick = connection
        .sync_manager
        .interpolation_tick(tick_manager.as_ref());
    let current_overstep = connection
        .sync_manager
        .interpolation_overstep(tick_manager.as_ref());
    for (confirmed_entity, confirmed_component) in confirmed_entities.iter() {
        if let Some(p) = confirmed_entity.interpolated {
            if let Ok(interpolated_entity) = interpolated_entities.get(p) {
                if confirmed_component.is_added() {
                    // safety: we know the entity exists
                    let mut interpolated_entity_mut =
                        commands.get_entity(interpolated_entity).unwrap();
                    // insert history
                    let history = ConfirmedHistory::<C>::new();
                    // map any entities from confirmed to interpolated
                    let mut new_component = confirmed_component.deref().clone();
                    component_registry.map_entities(
                        &mut new_component,
                        &mut manager.interpolated_entity_map.confirmed_to_interpolated,
                    );
                    match component_registry.interpolation_mode::<C>() {
                        ComponentSyncMode::Full => {
                            trace!(?interpolated_entity, tick=?tick_manager.tick(),  "spawn interpolation history");
                            interpolated_entity_mut.insert((
                                // NOTE: we probably do NOT want to insert the component right away, instead we want to wait until we have two updates
                                //  we can interpolate between. Otherwise it will look jarring if send_interval is low. (because the entity will
                                //  stay fixed until we get the next update, then it will start moving)
                                // new_component,
                                history,
                                InterpolateStatus::<C> {
                                    start: Some((current_tick, new_component)),
                                    end: None,
                                    current_tick,
                                    current_overstep,
                                },
                            ));
                        }
                        ComponentSyncMode::Once | ComponentSyncMode::Simple => {
                            debug!("copy interpolation component");
                            interpolated_entity_mut.insert(new_component);
                        }
                        ComponentSyncMode::None => {}
                    }
                }
            }
        }
    }
}

/// When we receive a server update for an interpolated component, we need to store it in the confirmed history,
pub(crate) fn apply_confirmed_update_mode_full<C: SyncComponent>(
    component_registry: Res<ComponentRegistry>,
    // TODO: unfortunately we need this to be mutable because of the MapEntities trait even though it's not actually needed...
    mut manager: ResMut<InterpolationManager>,
    mut interpolated_entities: Query<
        &mut ConfirmedHistory<C>,
        (With<Interpolated>, Without<Confirmed>),
    >,
    confirmed_entities: Query<(Entity, &Confirmed, Ref<C>)>,
) {
    let kind = std::any::type_name::<C>();
    for (confirmed_entity, confirmed, confirmed_component) in confirmed_entities.iter() {
        if let Some(p) = confirmed.interpolated {
            if confirmed_component.is_changed() && !confirmed_component.is_added() {
                if let Ok(mut history) = interpolated_entities.get_mut(p) {
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
                    component_registry.map_entities(
                        &mut component,
                        &mut manager.interpolated_entity_map.confirmed_to_interpolated,
                    );
                    trace!(?kind, tick = ?tick, "adding confirmed update to history");
                    // update the history at the value that the entity currently is
                    history.buffer.add_item(tick, component);

                    // TODO: here we do not want to update directly the component, that will be done during interpolation
                }
            }
        }
    }
}

/// When we receive a server update for a simple component, we just update the entity directly
pub(crate) fn apply_confirmed_update_mode_simple<C: SyncComponent>(
    component_registry: Res<ComponentRegistry>,
    // TODO: unfortunately we need this to be mutable because of the MapEntities trait even though it's not actually needed...
    mut manager: ResMut<InterpolationManager>,
    mut interpolated_entities: Query<&mut C, (With<Interpolated>, Without<Confirmed>)>,
    confirmed_entities: Query<(Entity, &Confirmed, Ref<C>)>,
) {
    for (confirmed_entity, confirmed, confirmed_component) in confirmed_entities.iter() {
        if let Some(p) = confirmed.interpolated {
            if confirmed_component.is_changed() && !confirmed_component.is_added() {
                if let Ok(mut interpolated_component) = interpolated_entities.get_mut(p) {
                    // for sync-components, we just match the confirmed component
                    // map any entities from confirmed to interpolated first
                    let mut component = confirmed_component.deref().clone();
                    component_registry.map_entities(
                        &mut component,
                        &mut manager.interpolated_entity_map.confirmed_to_interpolated,
                    );
                    *interpolated_component = component;
                }
            }
        }
    }
}
