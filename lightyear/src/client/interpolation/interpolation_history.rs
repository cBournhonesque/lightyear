use std::ops::Deref;

use bevy::prelude::{
    Commands, Component, DetectChanges, Entity, EventReader, Query, Ref, Res, ResMut, With, Without,
};
use tracing::{debug, error, info, trace};

use crate::client::components::Confirmed;
use crate::client::components::{ComponentSyncMode, SyncComponent};
use crate::client::events::ComponentUpdateEvent;
use crate::client::interpolation::interpolate::InterpolateStatus;
use crate::client::interpolation::Interpolated;
use crate::client::resource::Client;
use crate::protocol::Protocol;
use crate::shared::tick_manager::Tick;
use crate::utils::ready_buffer::ReadyBuffer;

/// To know if we need to do rollback, we need to compare the interpolated entity's history with the server's state updates
#[derive(Component, Debug)]
pub struct ConfirmedHistory<T: SyncComponent> {
    // TODO: here we can use a sequence buffer. We won't store more than a couple

    // TODO: add a max size for the buffer
    // We want to avoid using a SequenceBuffer for optimization (we don't want to store a copy of the component for each history tick)
    // We can afford to use a ReadyBuffer because we will get server updates with monotically increasing ticks
    // therefore we can get rid of the old ticks before the server update

    // We will only store the history for the ticks where the component got updated
    pub buffer: ReadyBuffer<Tick, T>,
}

impl<T: SyncComponent> Default for ConfirmedHistory<T> {
    fn default() -> Self {
        Self::new()
    }
}

// mostly used for tests
impl<T: SyncComponent> PartialEq for ConfirmedHistory<T> {
    fn eq(&self, other: &Self) -> bool {
        self.buffer.heap.iter().eq(other.buffer.heap.iter())
    }
}

impl<T: SyncComponent> ConfirmedHistory<T> {
    pub fn new() -> Self {
        Self {
            buffer: ReadyBuffer::new(),
        }
    }

    /// Reset the history for this component
    pub(crate) fn clear(&mut self) {
        self.buffer = ReadyBuffer::new();
    }

    pub(crate) fn peek(&mut self) -> Option<(Tick, &T)> {
        self.buffer.heap.peek().map(|item| (item.key, &item.item))
    }

    pub(crate) fn pop(&mut self) -> Option<(Tick, T)> {
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
        self.buffer.pop_until(&tick)
    }
}

// TODO: maybe add the component history on the Confirmed entity instead of Interpolated? would make more sense maybe
pub(crate) fn add_component_history<T: SyncComponent, P: Protocol>(
    mut commands: Commands,
    client: ResMut<Client<P>>,
    interpolated_entities: Query<Entity, (Without<ConfirmedHistory<T>>, With<Interpolated>)>,
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
                    let history = ConfirmedHistory::<T>::new();
                    match T::mode() {
                        ComponentSyncMode::Full => {
                            debug!("spawn interpolation history");
                            interpolated_entity_mut.insert((
                                // confirmed_component.deref().clone(),
                                history,
                                InterpolateStatus::<T> {
                                    start: None,
                                    end: None,
                                    current: client.interpolation_tick(),
                                },
                            ));
                        }
                        _ => {
                            debug!("copy interpolation component");
                            // interpolated_entity_mut.insert(confirmed_component.deref().clone());
                        }
                    }
                }
            }
        }
    }
}

/// When we receive a server update, we need to store it in the confirmed history,
/// or update the interpolated component directly if InterpolatedComponentMode::Sync
pub(crate) fn apply_confirmed_update<T: SyncComponent, P: Protocol>(
    client: Res<Client<P>>,
    mut interpolated_entities: Query<
        // TODO: handle missing T?
        (&mut T, Option<&mut ConfirmedHistory<T>>),
        (With<Interpolated>, Without<Confirmed>),
    >,
    confirmed_entities: Query<(Entity, &Confirmed, Ref<T>)>,
) {
    for (confirmed_entity, confirmed, confirmed_component) in confirmed_entities.iter() {
        if let Some(p) = confirmed.interpolated {
            if confirmed_component.is_changed() {
                if let Ok((mut interpolated_component, history_option)) =
                    interpolated_entities.get_mut(p)
                {
                    match T::mode() {
                        ComponentSyncMode::Full => {
                            let Some(mut history) = history_option else {
                                error!(
                                    "Interpolated entity {:?} doesn't have a ComponentHistory",
                                    p
                                );
                                continue;
                            };
                            let Some(channel) = client
                                .replication_manager()
                                .channel_by_local(confirmed_entity)
                            else {
                                error!(
                                    "Could not find replication channel for entity {:?}",
                                    confirmed_entity
                                );
                                continue;
                            };
                            info!(component = ?confirmed_component.name(), tick = ?channel.latest_tick, "adding confirmed update to history");
                            // assign the history at the value that the entity currently is
                            // TODO: think about mapping entities!
                            history
                                .buffer
                                .add_item(channel.latest_tick, confirmed_component.deref().clone());
                        }
                        // for sync-components, we just match the confirmed component
                        ComponentSyncMode::Simple => {
                            // TODO: think about mapping entities!
                            // *interpolated_component = confirmed_component.deref().clone();
                        }
                        _ => {}
                    }
                }
            }
        }
    }
}
