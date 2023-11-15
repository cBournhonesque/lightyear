use bevy::prelude::{
    Added, Commands, Component, DetectChanges, Entity, Query, Ref, Res, ResMut, With,
};
use std::ops::Deref;

use crate::client::Client;
use crate::tick::Tick;
use crate::{Protocol, ReadyBuffer};

use super::{Confirmed, Predicted, PredictedComponent, Rollback, RollbackState};

/// To know if we need to do rollback, we need to compare the predicted entity's history with the server's state updates

#[derive(Component)]
pub struct ComponentHistory<T: PredictedComponent> {
    // TODO: add a max size for the buffer
    // We want to avoid using a SequenceBuffer for optimization (we don't want to store a copy of the component for each history tick)
    // We can afford to use a ReadyBuffer because we will get server updates with monotically increasing ticks
    // therefore we can get rid of the old ticks before the server update

    // We will only store the history for the ticks where the component got updated
    pub buffer: ReadyBuffer<Tick, T>,
}

impl<T: PredictedComponent> ComponentHistory<T> {
    fn new() -> Self {
        Self {
            buffer: ReadyBuffer::new(),
        }
    }

    /// Reset the history for this component
    pub(crate) fn clear(&mut self) {
        self.buffer = ReadyBuffer::new();
    }

    /// Get the value of the component at the specified tick.
    /// Clears the history buffer of all ticks older than the specified tick.
    /// Returns None
    pub(crate) fn get_history_at_tick(&mut self, tick: Tick) -> Option<T> {
        if self.buffer.heap.is_empty() {
            return None;
        }
        let mut val = None;
        loop {
            if let Some(item_with_key) = self.buffer.heap.pop() {
                // we have a new update that is older than what we want, stop
                if item_with_key.key > tick {
                    // put back the update in the heap
                    self.buffer.heap.push(item_with_key);
                    break;
                } else {
                    val = Some(item_with_key.item);
                }
            } else {
                break;
            }
        }
        val
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Component, Clone, PartialEq, Eq, Debug)]
    pub struct A(u32);

    #[test]
    fn test_component_history() {
        let mut component_history = ComponentHistory::new();

        // check when we try to access a value when the buffer is empty
        assert_eq!(component_history.get_history_at_tick(Tick(0)), None);

        // check when we try to access an exact tick
        component_history.buffer.add_item(Tick(1), A(1));
        component_history.buffer.add_item(Tick(2), A(2));
        assert_eq!(component_history.get_history_at_tick(Tick(2)), Some(A(2)));
        // check that we cleared older ticks
        assert!(component_history.buffer.is_empty());

        // check when we try to access a value in-between ticks
        component_history.buffer.add_item(Tick(1), A(1));
        component_history.buffer.add_item(Tick(3), A(3));
        assert_eq!(component_history.get_history_at_tick(Tick(2)), Some(A(1)));
        assert_eq!(component_history.buffer.len(), 1);
        assert_eq!(component_history.get_history_at_tick(Tick(4)), Some(A(3)));
        assert!(component_history.buffer.is_empty());

        // check when we try to access a value before any ticks
        component_history.buffer.add_item(Tick(1), A(1));
        assert_eq!(component_history.get_history_at_tick(Tick(0)), None);
        assert_eq!(component_history.buffer.len(), 1);
    }
}

// TODO: maybe only add component_history for components that got replicated on confirmed?
//  also we need to copy the components from confirmed to predicted
// Copy component insert/remove from confirmed to predicted
// Currently we will just copy every PredictedComponent
pub fn add_component_history<T: PredictedComponent>(
    mut commands: Commands,
    predicted_entities: Query<Entity, Added<Predicted>>,
    confirmed_entities: Query<(&Confirmed, Ref<T>)>,
) {
    for (confirmed_entity, confirmed_component) in confirmed_entities.iter() {
        if confirmed_component.is_added() {
            // safety: we know the entity exists
            let mut predicted_entity_mut = commands.get_entity(confirmed_entity.predicted).unwrap();
            // and add the component state
            // insert an empty history, it will be filled by a rollback (since it starts empty)
            predicted_entity_mut.insert((
                confirmed_component.deref().clone(),
                ComponentHistory::<T>::new(),
            ));
        }
    }
}

/// After one fixed-update tick, we record the predicted component history for the current tick
pub fn update_component_history<T: PredictedComponent, P: Protocol>(
    mut query: Query<(Ref<T>, &mut ComponentHistory<T>)>,
    client: Res<Client<P>>,
    rollback: Res<Rollback>,
) {
    // tick for which we will record the history
    let tick = match rollback.state {
        // if not in rollback, we are recording the history for the current client tick
        RollbackState::Default => client.tick(),
        RollbackState::ShouldRollback { current_tick } => current_tick,
        RollbackState::DidRollback => {
            panic!("Should not be recording history after rollback")
        }
    };
    // record history
    // TODO: potentially change detection does not work during rollback!
    for (component, mut history) in query.iter_mut() {
        if component.is_changed() {
            history.buffer.add_item(tick, component.clone());
        }
    }
}
