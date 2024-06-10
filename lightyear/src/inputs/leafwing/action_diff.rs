use crate::prelude::{Deserialize, LeafwingUserAction, Serialize, Tick};
use bevy::math::Vec2;
use bevy::prelude::{Component, Entity, Event, Reflect, Resource};
use bevy::utils::HashMap;
use leafwing_input_manager::action_state::ActionState;
use leafwing_input_manager::axislike::DualAxisData;
use std::collections::VecDeque;

/// Will store an `ActionDiff` as well as what generated it (either an Entity, or nothing if the
/// input actions are represented by a `Resource`)
///
/// These are typically accessed using the `Events<ActionDiffEvent>` resource.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Event)]
pub struct ActionDiffEvent<A> {
    /// If some: the entity that has the `ActionState<A>` component
    /// If none: `ActionState<A>` is a Resource, not a component
    pub owner: Option<Entity>,
    /// The `ActionDiff` that was generated
    pub action_diff: Vec<ActionDiff<A>>,
}

/// Stores presses and releases of buttons without timing information
///
/// These are typically accessed using the `Events<ActionDiffEvent>` resource.
/// Uses a minimal storage format, in order to facilitate transport over the network.
///
/// An `ActionState` can be fully reconstructed from a stream of `ActionDiff`
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Reflect)]
pub enum ActionDiff<A> {
    /// The action was pressed
    Pressed {
        /// The value of the action
        action: A,
    },
    /// The action was released
    Released {
        /// The value of the action
        action: A,
    },
    /// The value of the action changed
    ValueChanged {
        /// The value of the action
        action: A,
        /// The new value of the action
        value: f32,
    },
    /// The axis pair of the action changed
    AxisPairChanged {
        /// The value of the action
        action: A,
        /// The new value of the axis
        axis_pair: Vec2,
    },
}

impl<A: LeafwingUserAction> ActionDiff<A> {
    pub(crate) fn action(&self) -> A {
        match self {
            ActionDiff::Pressed { action } => action.clone(),
            ActionDiff::Released { action } => action.clone(),
            ActionDiff::ValueChanged { action, value: _ } => action.clone(),
            ActionDiff::AxisPairChanged {
                action,
                axis_pair: _,
            } => action.clone(),
        }
    }

    /// Applies an [`ActionDiff`] (usually received over the network) to the [`ActionState`].
    ///
    /// This lets you reconstruct an [`ActionState`] from a stream of [`ActionDiff`]s
    pub(crate) fn apply(self, action_state: &mut ActionState<A>) {
        match self {
            ActionDiff::Pressed { action } => {
                action_state.press(&action);
                // Pressing will initialize the ActionData if it doesn't exist
                action_state.action_data_mut(&action).unwrap().value = 1.0;
            }
            ActionDiff::Released { action } => {
                action_state.release(&action);
                // Releasing will initialize the ActionData if it doesn't exist
                let action_data = action_state.action_data_mut(&action).unwrap();
                action_data.value = 0.;
                action_data.axis_pair = None;
            }
            ActionDiff::ValueChanged { action, value } => {
                action_state.press(&action);
                // Pressing will initialize the ActionData if it doesn't exist
                action_state.action_data_mut(&action).unwrap().value = value;
            }
            ActionDiff::AxisPairChanged { action, axis_pair } => {
                action_state.press(&action);
                // Pressing will initialize the ActionData if it doesn't exist
                let action_data = action_state.action_data_mut(&action).unwrap();
                action_data.axis_pair = Some(DualAxisData::from_xy(axis_pair));
                action_data.value = axis_pair.length();
            }
        };
    }
}

/// The `ActionDiffBuffer` stores the ActionDiff generated at each tick on the client.
///
/// It is used to send a more compact `InputMessage` to the server.
#[derive(Resource, Component, Debug)]
pub(crate) struct ActionDiffBuffer<A: LeafwingUserAction> {
    pub(crate) start_tick: Option<Tick>,
    buffer: VecDeque<HashMap<A, ActionDiff<A>>>,
}

impl<A: LeafwingUserAction> Default for ActionDiffBuffer<A> {
    fn default() -> Self {
        Self {
            start_tick: None,
            buffer: VecDeque::new(),
        }
    }
}

impl<A: LeafwingUserAction> ActionDiffBuffer<A> {
    pub(crate) fn end_tick(&self) -> Tick {
        self.start_tick.map_or(Tick(0), |start_tick| {
            start_tick + (self.buffer.len() as i16 - 1)
        })
    }

    /// Take the ActionDiff generated in the frame and use them to populate the buffer
    /// Note that multiple frame can use the same tick, in which case we will use the latest ActionDiff events
    /// for a given action
    pub(crate) fn set(&mut self, tick: Tick, diffs: &Vec<ActionDiff<A>>) {
        let diffs = diffs
            .iter()
            .map(|diff| (diff.action(), diff.clone()))
            .collect();
        let Some(start_tick) = self.start_tick else {
            // initialize the buffer
            self.start_tick = Some(tick);
            self.buffer.push_back(diffs);
            return;
        };

        // cannot set lower values than start_tick
        if tick < start_tick {
            return;
        }

        let end_tick = start_tick + (self.buffer.len() as i16 - 1);
        if tick > end_tick {
            // fill the ticks between end_tick and tick with a copy of the current ActionState
            for _ in 0..(tick - end_tick - 1) {
                self.buffer.push_back(HashMap::default());
            }
            // add a new value to the buffer, which we will override below
            self.buffer.push_back(diffs);
            return;
        }
        // safety: we are guaranteed that the tick is in the buffer
        let entry = self.buffer.get_mut((tick - start_tick) as usize).unwrap();

        // we could have multiple ActionDiff events for the same entity, because the events were generated in different frames
        // in which case we want to merge them
        // TODO: should we handle when we have multiple ActionDiff that cancel each other? It should be fine
        //  since we read the ActionDiff in order, so the later one will cancel the earlier one
        entry.extend(diffs);
    }

    /// Remove all the diffs that are older than the given tick, then return the diffs
    /// for the given tick
    pub(crate) fn pop(&mut self, tick: Tick) -> Vec<ActionDiff<A>> {
        let Some(start_tick) = self.start_tick else {
            return vec![];
        };
        if tick < start_tick {
            return vec![];
        }
        if tick > start_tick + (self.buffer.len() as i16 - 1) {
            // pop everything
            self.buffer = VecDeque::new();
            self.start_tick = Some(tick + 1);
            return vec![];
        }

        for _ in 0..(tick - start_tick) {
            // front is the oldest value
            self.buffer.pop_front();
        }
        self.start_tick = Some(tick + 1);

        self.buffer
            .pop_front()
            .map(|v| v.into_values().collect())
            .unwrap_or(vec![])
    }

    /// Get the ActionState for the given tick
    pub(crate) fn get(&self, tick: Tick) -> Vec<ActionDiff<A>> {
        let Some(start_tick) = self.start_tick else {
            return vec![];
        };
        if tick < start_tick || tick > start_tick + (self.buffer.len() as i16 - 1) {
            return vec![];
        }
        self.buffer
            .get((tick - start_tick) as usize)
            .map(|v| v.values().cloned().collect())
            .unwrap_or(vec![])
    }
    pub(crate) fn update_from_message(&mut self, end_tick: Tick, diffs: &Vec<Vec<ActionDiff<A>>>) {
        let message_start_tick = end_tick - diffs.len() as u16 + 1;
        for (delta, diffs_for_tick) in diffs.iter().enumerate() {
            let tick = message_start_tick + Tick(delta as u16);
            self.set(tick, diffs_for_tick);
        }
    }
}

#[cfg(test)]
mod tests {
    use bevy::prelude::Reflect;
    use leafwing_input_manager::Actionlike;

    use super::*;

    #[derive(
        Serialize, Deserialize, Copy, Clone, Eq, PartialEq, Debug, Hash, Reflect, Actionlike,
    )]
    enum Action {
        Jump,
    }

    #[test]
    fn test_update_from_message() {
        let mut diff_buffer = ActionDiffBuffer::default();

        let end_tick = Tick(20);
        let diffs = vec![
            vec![],
            vec![ActionDiff::Pressed {
                action: Action::Jump,
            }],
            vec![],
            vec![],
            vec![],
            vec![ActionDiff::Pressed {
                action: Action::Jump,
            }],
            vec![],
            vec![],
            vec![],
        ];

        diff_buffer.update_from_message(end_tick, &diffs);

        assert_eq!(diff_buffer.get(Tick(20)), vec![]);
        assert_eq!(diff_buffer.get(Tick(19)), vec![]);
        assert_eq!(diff_buffer.get(Tick(18)), vec![]);
        assert_eq!(
            diff_buffer.get(Tick(17)),
            vec![ActionDiff::Pressed {
                action: Action::Jump
            }]
        );
        assert_eq!(diff_buffer.get(Tick(16)), vec![]);
        assert_eq!(diff_buffer.get(Tick(15)), vec![]);
        assert_eq!(diff_buffer.get(Tick(14)), vec![]);
        assert_eq!(
            diff_buffer.get(Tick(13)),
            vec![ActionDiff::Pressed {
                action: Action::Jump
            }]
        );
        assert_eq!(diff_buffer.get(Tick(12)), vec![]);
    }
}
