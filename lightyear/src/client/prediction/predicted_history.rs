use std::ops::Deref;

use bevy::prelude::{
    Commands, Component, DetectChanges, Entity, Or, Query, Ref, RemovedComponents, Res, ResMut,
    With, Without,
};
use tracing::{debug, error};

use crate::client::components::{SyncComponent, SyncMetadata};
use crate::client::prediction::resource::PredictionManager;
use crate::prelude::{Named, PreSpawnedPlayerObject, ShouldBePredicted, TickManager};
use crate::protocol::Protocol;
use crate::shared::tick_manager::Tick;
use crate::utils::ready_buffer::ReadyBuffer;

use super::{ComponentSyncMode, Confirmed, Predicted, Rollback, RollbackState};

// TODO: maybe just option<T> ?
#[derive(Debug, PartialEq, Clone)]
pub enum ComponentState<T> {
    // the component got just removed
    Removed,
    // the component got updated
    Updated(T),
}

/// To know if we need to do rollback, we need to compare the predicted entity's history with the server's state updates
#[derive(Component, Debug)]
pub struct PredictionHistory<T: PartialEq> {
    // TODO: add a max size for the buffer
    // We want to avoid using a SequenceBuffer for optimization (we don't want to store a copy of the component for each history tick)
    // We can afford to use a ReadyBuffer because we will get server updates with monotically increasing ticks
    // therefore we can get rid of the old ticks before the server update

    // We will only store the history for the ticks where the component got updated
    pub buffer: ReadyBuffer<Tick, ComponentState<T>>,
}

impl<T: PartialEq> Default for PredictionHistory<T> {
    fn default() -> Self {
        Self {
            buffer: ReadyBuffer::new(),
        }
    }
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

impl<T: SyncComponent> PredictionHistory<T> {
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

// This system:
// - when we receive a confirmed entity, we will create a predicted entity
// - when that predicted entity is created, we need to copy all components from the confirmed entity to the predicted entity, and create ComponentHistories

// Copy component insert/remove from confirmed to predicted
// Currently we will just copy every PredictedComponent
// TODO: how to handle components that are synced between confirmed/predicted but are not replicated?
//  I guess they still need to be added here.
// TODO: add more options:
//  - copy component and add component history (for rollback)
//  - copy component to history and don't add component

// TODO: only run this for SyncComponent where SyncMode != None
#[allow(clippy::type_complexity)]
pub(crate) fn add_component_history<C: SyncComponent, P: Protocol>(
    // TODO: unfortunately we need this to be mutable because of the MapEntities trait even though it's not actually needed...
    mut manager: ResMut<PredictionManager>,
    mut commands: Commands,
    tick_manager: Res<TickManager>,
    predicted_entities: Query<
        (Entity, Option<Ref<C>>),
        (
            Without<PredictionHistory<C>>,
            // for all types of predicted entities, we want to add the component history to enable them to be rolled-back
            With<Predicted>,
        ),
    >,
    confirmed_entities: Query<(Entity, &Confirmed, Option<Ref<C>>)>,
) where
    P::Components: SyncMetadata<C>,
{
    let tick = tick_manager.tick();
    for (confirmed_entity, confirmed, confirmed_component) in confirmed_entities.iter() {
        if let Some(p) = confirmed.predicted {
            if let Ok((predicted_entity, predicted_component)) = predicted_entities.get(p) {
                // if component got added on predicted side, add history
                add_history::<C, P>(tick, predicted_entity, &predicted_component, &mut commands);

                // if component got added on confirmed side
                // - full: sync component and add history
                // - simple/once: sync component
                if let Some(confirmed_component) = confirmed_component {
                    if confirmed_component.is_added() {
                        debug!(kind = ?confirmed_component.name(), "Component added on confirmed side");
                        // safety: we know the entity exists
                        let mut predicted_entity_mut =
                            commands.get_entity(predicted_entity).unwrap();
                        // map any entities from confirmed to predicted
                        let mut new_component = confirmed_component.deref().clone();
                        new_component.map_entities(&mut manager.predicted_entity_map);
                        match P::Components::mode() {
                            ComponentSyncMode::Full => {
                                // insert history, it will be quickly filled by a rollback (since it starts empty before the current client tick)
                                // TODO: then there's no need to add the component here, since it's going to get added during rollback anyway
                                let mut history = PredictionHistory::<C>::default();
                                history.buffer.add_item(
                                    tick_manager.tick(),
                                    ComponentState::Updated(confirmed_component.deref().clone()),
                                );
                                predicted_entity_mut.insert((new_component, history));
                            }
                            ComponentSyncMode::Simple => {
                                debug!(kind = ?new_component.name(), "Component simple synced between confirmed and predicted");
                                // we only sync the components once, but we don't do rollback so no need for a component history
                                predicted_entity_mut.insert(new_component);
                            }
                            ComponentSyncMode::Once => {
                                // if this was a prespawned entity, don't override SyncMode::Once components!
                                if predicted_component.is_none() {
                                    // we only sync the components once, but we don't do rollback so no need for a component history
                                    predicted_entity_mut.insert(new_component);
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
    }
}

/// Add the history for prespawned entities.
/// This must run on FixedUpdate (for entities spawned on FixedUpdate and PreUpdate (for entities spawned on Update)
#[allow(clippy::type_complexity)]
pub fn add_prespawned_component_history<C: SyncComponent, P: Protocol>(
    mut commands: Commands,
    tick_manager: Res<TickManager>,
    prespawned_query: Query<
        (Entity, Option<Ref<C>>),
        (
            Without<PredictionHistory<C>>,
            Without<Confirmed>,
            // for pre-spawned entities
            Or<(With<ShouldBePredicted>, With<PreSpawnedPlayerObject>)>,
        ),
    >,
) where
    P::Components: SyncMetadata<C>,
{
    // add component history for pre-spawned entities right away
    for (predicted_entity, predicted_component) in prespawned_query.iter() {
        add_history::<C, P>(
            tick_manager.tick(),
            predicted_entity,
            &predicted_component,
            &mut commands,
        );
    }
}

/// Add history when a predicted component gets added
fn add_history<C: SyncComponent, P: Protocol>(
    tick: Tick,
    predicted_entity: Entity,
    predicted_component: &Option<Ref<C>>,
    commands: &mut Commands,
) where
    P::Components: SyncMetadata<C>,
{
    let kind = C::type_name();
    if P::Components::mode() == ComponentSyncMode::Full {
        if let Some(predicted_component) = predicted_component {
            // component got added on predicted side, add history
            if predicted_component.is_added() {
                debug!(?kind, ?tick, ?predicted_entity, "Adding prediction history");
                // insert history, it will be quickly filled by a rollback (since it starts empty before the current client tick)
                let mut history = PredictionHistory::<C>::default();
                history.buffer.add_item(
                    tick,
                    ComponentState::Updated(predicted_component.deref().clone()),
                );
                commands.entity(predicted_entity).insert(history);
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
pub fn update_prediction_history<T: SyncComponent>(
    mut query: Query<(Ref<T>, &mut PredictionHistory<T>)>,
    mut removed_component: RemovedComponents<T>,
    mut removed_entities: Query<&mut PredictionHistory<T>, Without<T>>,
    tick_manager: Res<TickManager>,
    rollback: Res<Rollback>,
) {
    // tick for which we will record the history
    let tick = match rollback.state {
        // if not in rollback, we are recording the history for the current client tick
        RollbackState::Default => tick_manager.tick(),
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
#[allow(clippy::type_complexity)]
pub(crate) fn apply_confirmed_update<C: SyncComponent, P: Protocol>(
    // TODO: unfortunately we need this to be mutable because of the MapEntities trait even though it's not actually needed...
    mut manager: ResMut<PredictionManager>,
    mut predicted_entities: Query<
        &mut C,
        (
            Without<PredictionHistory<C>>,
            Without<Confirmed>,
            With<Predicted>,
        ),
    >,
    confirmed_entities: Query<(&Confirmed, Ref<C>)>,
) where
    P::Components: SyncMetadata<C>,
{
    for (confirmed_entity, confirmed_component) in confirmed_entities.iter() {
        if let Some(p) = confirmed_entity.predicted {
            if confirmed_component.is_changed() && !confirmed_component.is_added() {
                if let Ok(mut predicted_component) = predicted_entities.get_mut(p) {
                    match P::Components::mode() {
                        ComponentSyncMode::Full => {
                            error!(
                                "The predicted entity {:?} should have a ComponentHistory",
                                p
                            );
                            unreachable!(
                                "This system should only run for ComponentSyncMode::Simple"
                            );
                        }
                        // for sync-components, we just match the confirmed component
                        ComponentSyncMode::Simple => {
                            // map any entities from confirmed to predicted
                            let mut component = confirmed_component.deref().clone();
                            component.map_entities(&mut manager.predicted_entity_map);
                            *predicted_component = component;
                        }
                        _ => {}
                    }
                }
            }
        }
    }
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
