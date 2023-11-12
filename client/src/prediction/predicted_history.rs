use crate::prediction::{PredictedComponent, Rollback};
use crate::Client;
use bevy::prelude::{Component, DetectChanges, Query, Ref, Res};
use lightyear_shared::tick::Tick;
use lightyear_shared::{Protocol, ReadyBuffer, SequenceBuffer};

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

    impl PredictedComponent for A {}

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

// TODO: system that adds component history for all predicted entities and predicted component

// TODO: potentially clear oldest ticks in history?
pub fn update_component_history<T: PredictedComponent, P: Protocol>(
    mut query: Query<(Ref<T>, &mut ComponentHistory<T>)>,
    client: Res<Client<P>>,
    rollback: Res<Rollback>,
) {
    // TODO: IF WE ARE STARTING THE ROLLBACK, WE NEED TO CLEAR THE HISTORIES!
    let tick = client.tick();
    for (component, mut history) in query.iter_mut() {
        if component.is_changed() {
            history.buffer.add_item(tick, component.clone());
        }
    }
}
