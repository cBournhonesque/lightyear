use crate::client::interpolation::interpolate::InterpolateStatus;
use crate::client::interpolation::{InterpolatedComponent, InterpolatedComponentMode};
use crate::client::prediction::Confirmed;
use crate::tick::Tick;
use crate::utils::ready_buffer::ItemWithReadyKey;
use crate::{Client, Protocol, ReadyBuffer};
use bevy::prelude::{Commands, Component, DetectChanges, Entity, Query, Ref, Res, Without};
use std::ops::Deref;
use tracing::error;

/// To know if we need to do rollback, we need to compare the interpolated entity's history with the server's state updates

#[derive(Component, Debug)]
pub struct ComponentHistory<T: InterpolatedComponent> {
    // TODO: here we can use a sequence buffer. We won't store more than a couple

    // TODO: add a max size for the buffer
    // We want to avoid using a SequenceBuffer for optimization (we don't want to store a copy of the component for each history tick)
    // We can afford to use a ReadyBuffer because we will get server updates with monotically increasing ticks
    // therefore we can get rid of the old ticks before the server update

    // We will only store the history for the ticks where the component got updated
    pub buffer: ReadyBuffer<Tick, T>,
}

// mostly used for tests
impl<T: InterpolatedComponent> PartialEq for ComponentHistory<T> {
    fn eq(&self, other: &Self) -> bool {
        self.buffer.heap.iter().eq(other.buffer.heap.iter())
    }
}

impl<T: InterpolatedComponent> ComponentHistory<T> {
    pub fn new() -> Self {
        Self {
            buffer: ReadyBuffer::new(),
        }
    }

    /// Reset the history for this component
    pub(crate) fn clear(&mut self) {
        self.buffer = ReadyBuffer::new();
    }

    pub(crate) fn pop_next(&mut self) -> Option<(Tick, T)> {
        self.buffer.heap.pop().map(|item| (item.key, item.item))
    }

    /// Get the value of the component at the specified tick.
    /// Clears the history buffer of all ticks older or equal than the specified tick.
    /// NOTE: doesn't pop the last value!
    /// Returns None
    /// CAREFUL:
    /// the component history will only contain the ticks where the component got updated, and otherwise
    /// contains gaps. Therefore, we need to always leave a value in the history buffer so that we can
    /// get the values for the future ticks
    pub(crate) fn pop_until_tick(&mut self, tick: Tick) -> Option<(Tick, T)> {
        self.buffer
            .pop_until(&tick)
            .map(|item| (item.key, item.item))
    }
}

// TODO: maybe add the component history on the Confirmed entity instead of Interpolated? would make more sense maybe
pub(crate) fn add_component_history<T: InterpolatedComponent, P: Protocol>(
    mut commands: Commands,
    client: Res<Client<P>>,
    interpolated_entities: Query<Entity, Without<ComponentHistory<T>>>,
    confirmed_entities: Query<(&Confirmed, Ref<T>)>,
) {
    for (confirmed_entity, confirmed_component) in confirmed_entities.iter() {
        if let Some(p) = confirmed_entity.interpolated {
            if let Ok(interpolated_entity) = interpolated_entities.get(p) {
                if confirmed_component.is_added() {
                    // safety: we know the entity exists
                    let mut interpolated_entity_mut =
                        commands.get_entity(interpolated_entity).unwrap();
                    // insert history
                    let mut history = ComponentHistory::<T>::new();
                    match T::mode() {
                        InterpolatedComponentMode::Interpolate => {
                            interpolated_entity_mut.insert((
                                confirmed_component.deref().clone(),
                                history,
                                InterpolateStatus::<T> {
                                    start: None,
                                    end: None,
                                    current: client.interpolated_tick(),
                                },
                            ));
                        }
                        _ => {
                            interpolated_entity_mut.insert(confirmed_component.deref().clone());
                        }
                    }
                }
            }
        }
    }
}

/// When we receive a server update, we need to store it in the component history,
/// or update the interpolated component directly if InterpolatedComponentMode::Sync
pub(crate) fn update_component_history<T: InterpolatedComponent, P: Protocol>(
    client: Res<Client<P>>,
    mut interpolated_entities: Query<(&mut T, Option<&mut ComponentHistory<T>>)>,
    confirmed_entities: Query<(&Confirmed, Ref<T>)>,
) {
    let latest_server_tick = client.latest_received_server_tick();
    for (confirmed_entity, confirmed_component) in confirmed_entities.iter() {
        if let Some(p) = confirmed_entity.interpolated {
            if confirmed_component.is_changed() {
                if let Ok((mut interpolated_component, mut history_option)) =
                    interpolated_entities.get_mut(p)
                {
                    match T::mode() {
                        InterpolatedComponentMode::Interpolate => {
                            if let Some(mut history) = history_option {
                                history.buffer.add_item(
                                    latest_server_tick,
                                    confirmed_component.deref().clone(),
                                );
                            } else {
                                error!(
                                    "Interpolated entity {:?} doesn't have a ComponentHistory",
                                    p
                                );
                            }
                        }
                        // for sync-components, we just match the confirmed component
                        InterpolatedComponentMode::Sync => {
                            *interpolated_component = confirmed_component.deref().clone();
                        }
                        InterpolatedComponentMode::CopyOnce => {}
                    }
                }
            }
        }
    }
}
