use std::ops::Deref;

use bevy::prelude::{
    Commands, Component, DetectChanges, Entity, Query, Ref, RemovedComponents, Res, With, Without,
};
use tracing::error;

use crate::client::components::SyncComponent;
use crate::client::Client;
use crate::tick::Tick;
use crate::{Protocol, ReadyBuffer};

use super::{ComponentSyncMode, Confirmed, Predicted, Rollback, RollbackState};

/// To know if we need to do rollback, we need to compare the predicted entity's history with the server's state updates
#[derive(Component, Debug)]
pub struct PredictionHistory<T: SyncComponent> {
    // TODO: add a max size for the buffer
    // We want to avoid using a SequenceBuffer for optimization (we don't want to store a copy of the component for each history tick)
    // We can afford to use a ReadyBuffer because we will get server updates with monotically increasing ticks
    // therefore we can get rid of the old ticks before the server update

    // We will only store the history for the ticks where the component got updated
    pub buffer: ReadyBuffer<Tick, ComponentState<T>>,
}

impl<T: SyncComponent> PartialEq for PredictionHistory<T> {
    fn eq(&self, other: &Self) -> bool {
        let mut self_history: Vec<_> = self.buffer.heap.iter().collect();
        let mut other_history: Vec<_> = other.buffer.heap.iter().collect();
        self_history.sort_by_key(|item| item.key);
        other_history.sort_by_key(|item| item.key);
        self_history.eq(&other_history)
    }
}

// TODO: maybe just option<T> ?
#[derive(Debug, PartialEq, Clone)]
pub enum ComponentState<T: SyncComponent> {
    // the component got just removed
    Removed,
    // the component got updated
    Updated(T),
}

impl<T: SyncComponent> PredictionHistory<T> {
    pub fn new() -> Self {
        Self {
            buffer: ReadyBuffer::new(),
        }
    }

    /// Reset the history for this component
    pub(crate) fn clear(&mut self) {
        self.buffer = ReadyBuffer::new();
    }

    /// Get the value of the component at the specified tick.
    /// Clears the history buffer of all ticks older or equal than the specified tick.
    /// NOTE: Stores the returned value in the provided tick!!!
    ///
    /// CAREFUL:
    /// the component history will only contain the ticks where the component got updated, and otherwise
    /// contains gaps. Therefore, we need to always leave a value in the history buffer so that we can
    /// get the values for the future ticks
    pub(crate) fn pop_until_tick(&mut self, tick: Tick) -> Option<ComponentState<T>> {
        self.buffer.pop_until(&tick).map(|(tick, state)| {
            // TODO: this clone is pretty bad and avoidable. Probably switch to a sequence buffer?
            self.buffer.add_item(tick, state.clone());
            state
        })
    }

    // /// Get the value of the component at the specified tick.
    // /// Clears the history buffer of all ticks older than the specified tick.
    // /// Returns None
    // pub(crate) fn get_history_at_tick(&mut self, tick: Tick) -> Option<T> {
    //     if self.buffer.heap.is_empty() {
    //         return None;
    //     }
    //     let mut val = None;
    //     loop {
    //         if let Some(item_with_key) = self.buffer.heap.pop() {
    //             // we have a new update that is older than what we want, stop
    //             if item_with_key.key > tick {
    //                 // put back the update in the heap
    //                 self.buffer.heap.push(item_with_key);
    //                 break;
    //             } else {
    //                 val = Some(item_with_key.item);
    //             }
    //         } else {
    //             break;
    //         }
    //     }
    //     val
    // }
}

#[cfg(test)]
mod tests {
    // use super::*;
    //
    // #[derive(Component, Clone, PartialEq, Eq, Debug)]
    // pub struct A(u32);
    //
    // #[test]
    // fn test_component_history() {
    //     let mut component_history = ComponentHistory::new();
    //
    //     // check when we try to access a value when the buffer is empty
    //     assert_eq!(component_history.get_history_at_tick(Tick(0)), None);
    //
    //     // check when we try to access an exact tick
    //     component_history.buffer.add_item(Tick(1), A(1));
    //     component_history.buffer.add_item(Tick(2), A(2));
    //     assert_eq!(component_history.get_history_at_tick(Tick(2)), Some(A(2)));
    //     // check that we cleared older ticks
    //     assert!(component_history.buffer.is_empty());
    //
    //     // check when we try to access a value in-between ticks
    //     component_history.buffer.add_item(Tick(1), A(1));
    //     component_history.buffer.add_item(Tick(3), A(3));
    //     assert_eq!(component_history.get_history_at_tick(Tick(2)), Some(A(1)));
    //     assert_eq!(component_history.buffer.len(), 1);
    //     assert_eq!(component_history.get_history_at_tick(Tick(4)), Some(A(3)));
    //     assert!(component_history.buffer.is_empty());
    //
    //     // check when we try to access a value before any ticks
    //     component_history.buffer.add_item(Tick(1), A(1));
    //     assert_eq!(component_history.get_history_at_tick(Tick(0)), None);
    //     assert_eq!(component_history.buffer.len(), 1);
    // }
}

// This system:
// - when we receive a confirmed entity, we will create a predicted entity
// - when that predicted entity is created, we need to copy all components from the confirmed entity to the predicted entity, and create ComponentHistories

// Copy component insert/remove from confirmed to predicted
// Currently we will just copy every PredictedComponent
// TODO: add more options:
//  - copy component and add component history (for rollback)
//  - copy component to history and don't add component
pub fn add_component_history<T: SyncComponent, P: Protocol>(
    mut commands: Commands,
    client: Res<Client<P>>,
    predicted_entities: Query<(Entity, Option<Ref<T>>), Without<PredictionHistory<T>>>,
    confirmed_entities: Query<(&Confirmed, Option<Ref<T>>)>,
) {
    for (confirmed_entity, confirmed_component) in confirmed_entities.iter() {
        if let Some(p) = confirmed_entity.predicted {
            if let Ok((predicted_entity, predicted_component)) = predicted_entities.get(p) {
                if let Some(predicted_component) = predicted_component {
                    if predicted_component.is_added() {
                        // safety: we know the entity exists
                        let mut predicted_entity_mut =
                            commands.get_entity(predicted_entity).unwrap();

                        // insert history, it will be quickly filled by a rollback (since it starts empty before the current client tick)
                        let mut history = PredictionHistory::<T>::new();
                        history.buffer.add_item(
                            client.tick(),
                            ComponentState::Updated(predicted_component.deref().clone()),
                        );
                        match T::mode() {
                            ComponentSyncMode::Full => {
                                predicted_entity_mut.insert(history);
                            }
                            // we don't do prediction/rollback so no need for a prediction history
                            _ => {}
                        }
                    }
                }
                if let Some(confirmed_component) = confirmed_component {
                    if confirmed_component.is_added() {
                        // safety: we know the entity exists
                        let mut predicted_entity_mut =
                            commands.get_entity(predicted_entity).unwrap();
                        // insert history, it will be quickly filled by a rollback (since it starts empty before the current client tick)
                        let mut history = PredictionHistory::<T>::new();
                        match T::mode() {
                            ComponentSyncMode::Full => {
                                predicted_entity_mut
                                    .insert((confirmed_component.deref().clone(), history));
                            }
                            _ => {
                                // we only sync the components once, but we don't do rollback so no need for a component history
                                predicted_entity_mut.insert(confirmed_component.deref().clone());
                            }
                        }
                    }
                }
            }
        }
    }
}

// here I wanted to remove the component-history from predicted if it gets removed from confirmed, but maybe there's no need.
// pub fn remove_component_history<T: PredictedComponent>(
//     mut commands: Commands,
//     confirmed_entities: Query<&Confirmed>,
//     mut removed_component: RemovedComponents<T>,
// ) {
//     removed_component.read().for_each(|entity| {
//         if let Ok(confirmed) = confirmed_entities.get(entity) {
//             // the confirmed entity got the component removed, so we need to remove it from the predicted entity
//         }
//     })
// }

// 4 cases:
// - comp is alive in confirmed and predicted: OK
// - comp is alive in confirmed, but gets removed in pred at some point:
//   - we need still keep around the component-history to potentially do rollback.
//   - we could try to remove the history when the component gets removed from confirmed, but then how do we do rollback later?
//     - if the component gets added to the confirmed but didn't exist in predicted at the history (i.e. there's no history) -> we need to add a history whenever we receive the component
//       on confirmed OR we rollback if component exists on confirm but there is no history on predicted
//     - if the component gets added to the predicted but didn't exist in confirmed -> we need to add a history whenever we add the component to predicted!
//   - we could also just keep the history around, no biggie?
//

// rollback table:
// - confirm exist. rollback if:
//    - predicted history exists and is different
//    - predicted history does not exist
//    To rollback:
//    - update the predicted component to the confirmed component if it exists
//    - insert the confirmed component to the predicted entity if it doesn't exist
// - confirm does not exist. rollback if:
//    - predicted history exists and doesn't contain Removed
//    -
//    To rollback:
//    - we remove the component from predicted.

/// After one fixed-update tick, we record the predicted component history for the current tick
pub fn update_prediction_history<T: SyncComponent, P: Protocol>(
    mut query: Query<(Ref<T>, &mut PredictionHistory<T>)>,
    mut removed_component: RemovedComponents<T>,
    mut removed_entities: Query<&mut PredictionHistory<T>, Without<T>>,
    client: Res<Client<P>>,
    rollback: Res<Rollback>,
) {
    // tick for which we will record the history
    let tick = match rollback.state {
        // if not in rollback, we are recording the history for the current client tick
        RollbackState::Default => client.tick(),
        // if in rollback, we are recording the history for the current rollback tick
        RollbackState::ShouldRollback { current_tick } => current_tick,
    };
    // update history if the predicted component changed
    // TODO: potentially change detection does not work during rollback!
    //  edit: looks like it does
    for (component, mut history) in query.iter_mut() {
        // change detection works even when running the schedule for rollback (with no time increase)
        if component.is_changed() {
            history
                .buffer
                .add_item(tick, ComponentState::Updated(component.clone()));
        }
    }
    for entity in removed_component.read() {
        if let Ok(mut history) = removed_entities.get_mut(entity) {
            history.buffer.add_item(tick, ComponentState::Removed);
        }
    }
}

/// When we receive a server update, we might want to apply it to the predicted entity
pub(crate) fn apply_confirmed_update<T: SyncComponent, P: Protocol>(
    client: Res<Client<P>>,
    mut predicted_entities: Query<
        (&mut T),
        (
            Without<PredictionHistory<T>>,
            Without<Confirmed>,
            With<Predicted>,
        ),
    >,
    confirmed_entities: Query<(&Confirmed, Ref<T>)>,
) {
    let latest_server_tick = client.latest_received_server_tick();
    for (confirmed_entity, confirmed_component) in confirmed_entities.iter() {
        if let Some(p) = confirmed_entity.predicted {
            if confirmed_component.is_changed() {
                if let Ok(mut predicted_component) = predicted_entities.get_mut(p) {
                    match T::mode() {
                        ComponentSyncMode::Full => {
                            error!(
                                "The predicted entity {:?} should have a ComponentHistory",
                                p
                            );
                        }
                        // for sync-components, we just match the confirmed component
                        ComponentSyncMode::Simple => {
                            *predicted_component = confirmed_component.deref().clone();
                        }
                        ComponentSyncMode::Once => {}
                    }
                }
            }
        }
    }
}
