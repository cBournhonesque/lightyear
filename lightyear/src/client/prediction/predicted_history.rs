//! Managed the history buffer, which is a buffer of the past predicted component states,
//! so that whenever we receive an update from the server we can compare the predicted entity's history with the server update.
use std::ops::Deref;

use bevy::prelude::{
    Commands, Component, DetectChanges, Entity, Or, Query, Ref, RemovedComponents, Res, ResMut,
    With, Without,
};
use tracing::{debug, error, info, trace};

use crate::client::components::{ComponentSyncMode, Confirmed, SyncComponent, SyncMetadata};
use crate::client::prediction::resource::PredictionManager;
use crate::client::prediction::rollback::{Rollback, RollbackState};
use crate::client::prediction::Predicted;
use crate::prelude::{
    ComponentRegistry, ExternalMapper, PreSpawnedPlayerObject, ShouldBePredicted, TickManager,
};
use crate::shared::tick_manager::Tick;
use crate::utils::ready_buffer::ReadyBuffer;

/// Stores a past update for a component
#[derive(Debug, PartialEq, Clone)]
pub(crate) enum ComponentState<C> {
    /// the component just got removed
    Removed,
    /// the component got updated
    Updated(C),
}

/// To know if we need to do rollback, we need to compare the predicted entity's history with the server's state updates
#[derive(Component, Debug)]
pub(crate) struct PredictionHistory<C: PartialEq> {
    // TODO: add a max size for the buffer
    // We want to avoid using a SequenceBuffer for optimization (we don't want to store a copy of the component for each history tick)
    // We can afford to use a ReadyBuffer because we will get server updates with monotonically increasing ticks
    // therefore we can get rid of the old ticks before the server update

    // We will only store the history for the ticks where the component got updated
    pub buffer: ReadyBuffer<Tick, ComponentState<C>>,
}

impl<C: PartialEq> Default for PredictionHistory<C> {
    fn default() -> Self {
        Self {
            buffer: ReadyBuffer::new(),
        }
    }
}

impl<C: SyncComponent> PartialEq for PredictionHistory<C> {
    fn eq(&self, other: &Self) -> bool {
        let mut self_history: Vec<_> = self.buffer.heap.iter().collect();
        let mut other_history: Vec<_> = other.buffer.heap.iter().collect();
        self_history.sort_by_key(|item| item.key);
        other_history.sort_by_key(|item| item.key);
        self_history.eq(&other_history)
    }
}

impl<C: SyncComponent> PredictionHistory<C> {
    /// Reset the history for this component
    pub(crate) fn clear(&mut self) {
        self.buffer = ReadyBuffer::new();
    }

    /// Add to the buffer that we received an update for the component at the given tick
    pub(crate) fn add_update(&mut self, tick: Tick, component: C) {
        self.buffer
            .add_item(tick, ComponentState::Updated(component));
    }

    /// Add to the buffer that the component got removed at the given tick
    pub(crate) fn add_remove(&mut self, tick: Tick) {
        self.buffer.add_item(tick, ComponentState::Removed);
    }

    // TODO: check if this logic is necessary/correct?
    /// Clear the history of values strictly older than the specified tick,
    /// and return the most recent value that is older or equal to the specified tick.
    /// NOTE: That value is written back into the buffer
    ///
    /// CAREFUL:
    /// the component history will only contain the ticks where the component got updated, and otherwise
    /// contains gaps. Therefore, we need to always leave a value in the history buffer so that we can
    /// get the values for the future ticks
    pub(crate) fn pop_until_tick(&mut self, tick: Tick) -> Option<ComponentState<C>> {
        self.buffer.pop_until(&tick).map(|(tick, state)| {
            // TODO: this clone is pretty bad and avoidable. Probably switch to a sequence buffer?
            self.buffer.add_item(tick, state.clone());
            state
        })
    }
}

// Copy component insert/remove from confirmed to predicted
// Currently we will just copy every PredictedComponent
// TODO: how to handle components that are synced between confirmed/predicted but are not replicated?
//  I guess they still need to be added here.
// TODO: add more options:
//  - copy component and add component history (for rollback)
//  - copy component to history and don't add component

// TODO: only run this for SyncComponent where SyncMode != None
/// Add component history for entities that are predicted
/// There is extra complexity because the component could get added on the Confirmed entity (received from the server), or added to the Predited entity directly
#[allow(clippy::type_complexity)]
pub(crate) fn add_component_history<C: SyncComponent>(
    component_registry: Res<ComponentRegistry>,
    manager: Res<PredictionManager>,
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
) {
    let kind = std::any::type_name::<C>();
    let tick = tick_manager.tick();
    for (confirmed_entity, confirmed, confirmed_component) in confirmed_entities.iter() {
        if let Some(p) = confirmed.predicted {
            if let Ok((predicted_entity, predicted_component)) = predicted_entities.get(p) {
                // if component got added on predicted side, add history
                add_history::<C>(
                    component_registry.as_ref(),
                    tick,
                    predicted_entity,
                    &predicted_component,
                    &mut commands,
                );

                // if component got added on confirmed side
                // - full: sync component and add history
                // - simple/once: sync component
                if let Some(confirmed_component) = confirmed_component {
                    if confirmed_component.is_added() {
                        trace!(?kind, "Component added on confirmed side");
                        // safety: we know the entity exists
                        let mut predicted_entity_mut =
                            commands.get_entity(predicted_entity).unwrap();
                        // map any entities from confirmed to predicted
                        let mut new_component = confirmed_component.deref().clone();
                        manager.map_entities(&mut new_component, component_registry.as_ref());
                        match component_registry.prediction_mode::<C>() {
                            ComponentSyncMode::Full => {
                                // insert history, it will be quickly filled by a rollback (since it starts empty before the current client tick)
                                // or will it? because the component just got spawned anyway..
                                // TODO: then there's no need to add the component here, since it's going to get added during rollback anyway?
                                let mut history = PredictionHistory::<C>::default();
                                history.add_update(tick, confirmed_component.deref().clone());
                                predicted_entity_mut.insert((new_component, history));
                            }
                            ComponentSyncMode::Simple => {
                                debug!(
                                    ?kind,
                                    "Component simple synced between confirmed and predicted"
                                );
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
/// This must run on FixedUpdate (for entities spawned on FixedUpdate) and PreUpdate (for entities spawned on Update)
#[allow(clippy::type_complexity)]
pub(crate) fn add_prespawned_component_history<C: SyncComponent>(
    component_registry: Res<ComponentRegistry>,
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
) {
    // add component history for pre-spawned entities right away
    for (predicted_entity, predicted_component) in prespawned_query.iter() {
        add_history::<C>(
            component_registry.as_ref(),
            tick_manager.tick(),
            predicted_entity,
            &predicted_component,
            &mut commands,
        );
    }
}

/// Add history when a predicted component gets added
fn add_history<C: SyncComponent>(
    component_registry: &ComponentRegistry,
    tick: Tick,
    predicted_entity: Entity,
    predicted_component: &Option<Ref<C>>,
    commands: &mut Commands,
) {
    let kind = std::any::type_name::<C>();
    if component_registry.prediction_mode::<C>() == ComponentSyncMode::Full {
        if let Some(predicted_component) = predicted_component {
            // component got added on predicted side, add history
            if predicted_component.is_added() {
                debug!(?kind, ?tick, ?predicted_entity, "Adding prediction history");
                // insert history, it will be quickly filled by a rollback (since it starts empty before the current client tick)
                let mut history = PredictionHistory::<C>::default();
                history.add_update(tick, predicted_component.deref().clone());
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
pub(crate) fn update_prediction_history<T: SyncComponent>(
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
            history.add_update(tick, component.deref().clone());
        }
    }
    for entity in removed_component.read() {
        if let Ok(mut history) = removed_entities.get_mut(entity) {
            history.add_remove(tick);
        }
    }
}

/// When we receive a server update, we might want to apply it to the predicted entity
#[allow(clippy::type_complexity)]
pub(crate) fn apply_confirmed_update<C: SyncComponent>(
    component_registry: Res<ComponentRegistry>,
    manager: Res<PredictionManager>,
    mut predicted_entities: Query<
        &mut C,
        (
            Without<PredictionHistory<C>>,
            Without<Confirmed>,
            With<Predicted>,
        ),
    >,
    confirmed_entities: Query<(&Confirmed, Ref<C>)>,
) {
    for (confirmed_entity, confirmed_component) in confirmed_entities.iter() {
        if let Some(p) = confirmed_entity.predicted {
            if confirmed_component.is_changed() && !confirmed_component.is_added() {
                if let Ok(mut predicted_component) = predicted_entities.get_mut(p) {
                    match component_registry.prediction_mode::<C>() {
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
                            manager.map_entities(&mut component, component_registry.as_ref());
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
    use super::*;
    use crate::tests::protocol::*;
    use crate::tests::stepper::{BevyStepper, Step};
    use crate::utils::ready_buffer::ItemWithReadyKey;

    /// Test adding and removing updates to the component history
    #[test]
    fn test_component_history() {
        let mut component_history = PredictionHistory::<Component1>::default();

        // check when we try to access a value when the buffer is empty
        assert_eq!(component_history.pop_until_tick(Tick(0)), None);

        // check when we try to access an exact tick
        component_history.add_update(Tick(1), Component1(1.0));
        component_history.add_update(Tick(2), Component1(2.0));
        assert_eq!(
            component_history.pop_until_tick(Tick(2)),
            Some(ComponentState::Updated(Component1(2.0)))
        );
        // check that we cleared older ticks, and that the most recent value still remains
        assert_eq!(component_history.buffer.len(), 1);
        assert!(component_history.buffer.has_item(&Tick(2)));

        // check when we try to access a value in-between ticks
        component_history.add_update(Tick(4), Component1(4.0));
        // we retrieve the most recent value older or equal to Tick(3)
        assert_eq!(
            component_history.pop_until_tick(Tick(3)),
            Some(ComponentState::Updated(Component1(2.0)))
        );
        assert_eq!(component_history.buffer.len(), 2);
        // check that the most recent value got added back to the buffer at the popped tick
        assert_eq!(
            component_history.buffer.heap.peek(),
            Some(&ItemWithReadyKey {
                key: Tick(2),
                item: ComponentState::Updated(Component1(2.0))
            })
        );
        assert!(component_history.buffer.has_item(&Tick(4)));

        // check that nothing happens when we try to access a value before any ticks
        assert_eq!(component_history.pop_until_tick(Tick(0)), None);
        assert_eq!(component_history.buffer.len(), 2);
    }

    /// Test adding the component history to the predicted entity
    /// 1. Add the history for ComponentSyncMode::Full that was added to the confirmed entity
    /// 2. Add the history for ComponentSyncMode::Full that was added to the predicted entity
    /// 3. Don't add the history for ComponentSyncMode::Simple
    /// 4. Don't add the history for ComponentSyncMode::Once
    // TODO: check map entities
    #[test]
    fn test_add_component_history() {
        let mut stepper = BevyStepper::default();

        let tick = stepper.client_tick();
        let confirmed = stepper.client_app.world.spawn(Confirmed::default()).id();
        let predicted = stepper
            .client_app
            .world
            .spawn(Predicted {
                confirmed_entity: Some(confirmed),
            })
            .id();
        stepper
            .client_app
            .world
            .entity_mut(confirmed)
            .get_mut::<Confirmed>()
            .unwrap()
            .predicted = Some(predicted);

        // 1. Add the history for ComponentSyncMode::Full that was added to the confirmed entity
        stepper
            .client_app
            .world
            .entity_mut(confirmed)
            .insert(Component1(1.0));
        stepper.frame_step();
        assert_eq!(
            stepper
                .client_app
                .world
                .entity_mut(predicted)
                .get_mut::<PredictionHistory<Component1>>()
                .expect("Expected prediction history to be added")
                .pop_until_tick(tick),
            Some(ComponentState::Updated(Component1(1.0))),
            "Expected component value to be added to prediction history"
        );
        assert_eq!(
            stepper
                .client_app
                .world
                .entity(predicted)
                .get::<Component1>()
                .expect("Expected component to be added to predicted entity"),
            &Component1(1.0),
            "Expected component to be added to predicted entity"
        );

        // 2. Add the history for ComponentSyncMode::Full that was added to the predicted entity
        let tick = stepper.client_tick();
        stepper
            .client_app
            .world
            .entity_mut(predicted)
            .insert(Component5(1.0));
        stepper.frame_step();
        assert_eq!(
            stepper
                .client_app
                .world
                .entity_mut(predicted)
                .get_mut::<PredictionHistory<Component5>>()
                .expect("Expected prediction history to be added")
                .pop_until_tick(tick),
            Some(ComponentState::Updated(Component5(1.0))),
            "Expected component value to be added to prediction history"
        );
        assert_eq!(
            stepper
                .client_app
                .world
                .entity(predicted)
                .get::<Component5>()
                .expect("Expected component to be added to predicted entity"),
            &Component5(1.0),
            "Expected component to be added to predicted entity"
        );

        // 3. Don't add the history for ComponentSyncMode::Simple
        let tick = stepper.client_tick();
        stepper
            .client_app
            .world
            .entity_mut(confirmed)
            .insert(Component2(1.0));
        stepper.frame_step();
        assert!(
            stepper
                .client_app
                .world
                .entity(predicted)
                .get::<PredictionHistory<Component2>>()
                .is_none(),
            "Expected component value to not be added to prediction history for ComponentSyncMode::Simple"
        );
        assert_eq!(
            stepper
                .client_app
                .world
                .entity(predicted)
                .get::<Component2>()
                .expect("Expected component to be added to predicted entity"),
            &Component2(1.0),
            "Expected component to be added to predicted entity"
        );

        // 4. Don't add the history for ComponentSyncMode::Once
        let tick = stepper.client_tick();
        stepper
            .client_app
            .world
            .entity_mut(confirmed)
            .insert(Component3(1.0));
        stepper.frame_step();
        assert!(
            stepper
                .client_app
                .world
                .entity(predicted)
                .get::<PredictionHistory<Component3>>()
                .is_none(),
            "Expected component value to not be added to prediction history for ComponentSyncMode::Once"
        );
        assert_eq!(
            stepper
                .client_app
                .world
                .entity(predicted)
                .get::<Component3>()
                .expect("Expected component to be added to predicted entity"),
            &Component3(1.0),
            "Expected component to be added to predicted entity"
        );
    }
}
