use std::collections::VecDeque;
use std::fmt::{Debug, Formatter};

use crate::inputs::leafwing::action_diff::ActionDiff;
use bevy::prelude::{Component, Resource};
use leafwing_input_manager::prelude::ActionState;
use serde::{Deserialize, Serialize};
use tracing::trace;

use crate::shared::tick_manager::Tick;

use super::LeafwingUserAction;

// NOTE: we can have multiple Actionlike, (each entity could have a different Actionlike),
//  so we will have a separate InputBuffer for each!

// CLIENT:
// - store the diffs for each past ticks
// - during rollback we can apply the diffs in reverse -> is this possible?
//   - if not possible, we just store the ActionState for each tick (a bit expensive...)
//   - should be ok if we pre-allocate

// SERVER:
// - we receive a message containing for each tick a list of diffs
// - we apply the ticks on the right tick to the entity/resource
// - no need to maintain our inputbuffer on the server

// NOTE: right now, for simplicity, we will send all the action-diffs for all entities in one single message.
// TODO: can we just use History<ActionState> then? why do we need a special component?
//  maybe because we want to send/store inputs even before we apply them
/// The InputBuffer contains a history of the ActionState
// TODO: improve this data structure
#[derive(Resource, Component, Debug)]
pub(crate) struct InputBuffer<A: LeafwingUserAction> {
    pub(crate) start_tick: Option<Tick>,
    pub(crate) buffer: VecDeque<BufferItem<ActionState<A>>>,
}
impl<A: LeafwingUserAction> std::fmt::Display for InputBuffer<A> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let ty = A::short_type_path();
        let buffer_str = self
            .buffer
            .iter()
            .map(|item| match item {
                BufferItem::Absent => "Absent".to_string(),
                BufferItem::SameAsPrecedent => "SameAsPrecedent".to_string(),
                BufferItem::Data(data) => format!("{:?}", data.get_pressed()),
            })
            .collect::<Vec<String>>()
            .join(", ");
        write!(
            f,
            "InputBuffer<{:?}> = Start tick: {:?}. Buffer: {:?}",
            ty, self.start_tick, buffer_str
        )
    }
}

// We use this to avoid cloning values in the buffer too much
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub(crate) enum BufferItem<T> {
    Absent,
    SameAsPrecedent,
    Data(T),
}

impl<A: LeafwingUserAction> Default for InputBuffer<A> {
    fn default() -> Self {
        Self {
            start_tick: None,
            buffer: VecDeque::new(),
        }
    }
}

impl<T: LeafwingUserAction> InputBuffer<T> {
    // Note: we expect this to be set every tick?
    //  i.e. there should be an ActionState for every tick, even if the action is None
    /// Set the ActionState for the given tick in the InputBuffer
    ///
    /// This should be called every tick.
    pub(crate) fn set(&mut self, tick: Tick, value: &ActionState<T>) {
        let Some(start_tick) = self.start_tick else {
            // initialize the buffer
            self.start_tick = Some(tick);
            self.buffer.push_back(BufferItem::Data(value.clone()));
            return;
        };

        // cannot set lower values than start_tick
        if tick < start_tick {
            return;
        }

        let end_tick = start_tick + (self.buffer.len() as i16 - 1);
        if tick > end_tick {
            // TODO: Think about how to fill the buffer between ticks
            //  - we want: if an input is missing, we consider that the user did the same action (RocketLeague or Overwatch GDC)

            // TODO: think about whether this is correct or not, it is correct if we always call set()
            //  with monotonically increasing ticks, which I think is the case
            //  maybe that's not correct because the timing information should be different? (i.e. I should tick the action-states myself and set them)
            // fill the ticks between end_tick and tick with a copy of the current ActionState
            for _ in 0..(tick - end_tick - 1) {
                trace!("fill ticks");
                self.buffer.push_back(BufferItem::SameAsPrecedent);
            }
            // add a new value to the buffer, which we will override below
            self.buffer.push_back(BufferItem::Absent);
        }

        // check if the value is the same as the precedent tick, in which case we compress it
        let mut same_as_precedent = false;
        if let Some(action_state) = self.get(tick - 1) {
            if action_state == value {
                same_as_precedent = true;
            }
        }

        // safety: we are guaranteed that the tick is in the buffer
        let entry = self.buffer.get_mut((tick - start_tick) as usize).unwrap();

        if same_as_precedent {
            *entry = BufferItem::SameAsPrecedent;
        } else {
            *entry = BufferItem::Data(value.clone());
        }
    }

    /// Remove all the inputs that are older than the given tick, then return the input
    /// for the given tick
    pub(crate) fn pop(&mut self, tick: Tick) -> Option<ActionState<T>> {
        let Some(start_tick) = self.start_tick else {
            return None;
        };
        if tick < start_tick {
            return None;
        }
        if tick > start_tick + (self.buffer.len() as i16 - 1) {
            // pop everything
            self.buffer = VecDeque::new();
            self.start_tick = Some(tick + 1);
            return None;
        }
        // info!(
        //     "buffer: {:?}. start_tick: {:?}, tick: {:?}",
        //     self.buffer, self.start_tick, tick
        // );

        // popped will represent the last value popped
        let mut popped = BufferItem::Absent;
        for _ in 0..(tick + 1 - start_tick) {
            // front is the oldest value
            let data = self.buffer.pop_front();
            if let Some(BufferItem::Data(value)) = data {
                popped = BufferItem::Data(value);
            }
        }
        self.start_tick = Some(tick + 1);

        // if the next value after we popped was 'SameAsPrecedent', we need to override it with an actual value
        if let Some(BufferItem::SameAsPrecedent) = self.buffer.front() {
            *self.buffer.front_mut().unwrap() = popped.clone();
        }

        if let BufferItem::Data(value) = popped {
            return Some(value);
        } else {
            return None;
        }
    }

    /// Get the ActionState for the given tick
    pub(crate) fn get(&self, tick: Tick) -> Option<&ActionState<T>> {
        let Some(start_tick) = self.start_tick else {
            return None;
        };
        if self.buffer.is_empty() {
            return None;
        }
        if tick < start_tick || tick > start_tick + (self.buffer.len() as i16 - 1) {
            return None;
        }
        let data = self.buffer.get((tick - start_tick) as usize).unwrap();
        match data {
            BufferItem::Absent => None,
            BufferItem::SameAsPrecedent => {
                // get the data from the preceding tick
                self.get(tick - 1)
            }
            BufferItem::Data(data) => Some(data),
        }
    }

    /// Get latest ActionState present in the buffer
    pub(crate) fn get_last(&self) -> Option<&ActionState<T>> {
        let Some(start_tick) = self.start_tick else {
            return None;
        };
        if self.buffer.is_empty() {
            return None;
        }
        self.get(start_tick + (self.buffer.len() as i16 - 1))
    }

    /// Upon receiving an [`InputMessage`], update the InputBuffer with the all the inputs
    /// included in the message.
    pub(crate) fn update_from_message(
        &mut self,
        end_tick: Tick,
        start_value: &ActionState<T>,
        diffs: &Vec<Vec<ActionDiff<T>>>,
    ) {
        let start_tick = end_tick - diffs.len() as u16;
        self.set(start_tick, start_value);

        let mut value = start_value.clone();
        for (delta, diffs_for_tick) in diffs.iter().enumerate() {
            let tick = start_tick + Tick(1 + delta as u16);
            for diff in diffs_for_tick {
                // TODO: also handle timings!
                diff.apply(&mut value);
            }
            self.set(tick, &value);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::inputs::leafwing::input_message::InputTarget;
    use crate::prelude::InputMessage;
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
    fn test_get_set_pop() {
        let mut input_buffer = InputBuffer::default();

        let mut a1 = ActionState::default();
        a1.press(&Action::Jump);
        a1.action_data_mut(&Action::Jump).unwrap().value = 0.0;
        let mut a2 = ActionState::default();
        a2.press(&Action::Jump);
        a1.action_data_mut(&Action::Jump).unwrap().value = 1.0;
        input_buffer.set(Tick(3), &a1);
        input_buffer.set(Tick(6), &a2);
        input_buffer.set(Tick(7), &a2);

        assert_eq!(input_buffer.start_tick, Some(Tick(3)));
        assert_eq!(input_buffer.buffer.len(), 5);

        assert_eq!(input_buffer.get(Tick(3)), Some(&a1));
        assert_eq!(input_buffer.get(Tick(4)), Some(&a1));
        assert_eq!(input_buffer.get(Tick(5)), Some(&a1));
        assert_eq!(input_buffer.get(Tick(6)), Some(&a2));
        assert_eq!(input_buffer.get(Tick(8)), None);

        assert_eq!(input_buffer.pop(Tick(4)), Some(a1.clone()));
        assert_eq!(input_buffer.start_tick, Some(Tick(5)));
        assert_eq!(input_buffer.buffer.len(), 3);

        // the oldest element has been updated from `SameAsPrecedent` to `Data`
        assert_eq!(
            input_buffer.buffer.front().unwrap(),
            &BufferItem::Data(a1.clone())
        );
        assert_eq!(input_buffer.pop(Tick(7)), Some(a2.clone()));
        assert_eq!(input_buffer.start_tick, Some(Tick(8)));
        assert_eq!(input_buffer.buffer.len(), 0);
    }

    #[test]
    fn test_update_from_message() {
        let mut input_buffer = InputBuffer::default();
        let input_message = InputMessage {
            end_tick: Tick(20),
            diffs: vec![(
                InputTarget::Global,
                ActionState::default(),
                vec![
                    vec![],
                    vec![ActionDiff::Pressed {
                        action: Action::Jump,
                    }],
                    vec![ActionDiff::Released {
                        action: Action::Jump,
                    }],
                ],
            )],
        };

        input_buffer.update_from_message()

        let mut a1 = ActionState::default();
        a1.press(&Action::Jump);
        a1.action_data_mut(&Action::Jump).unwrap().value = 0.0;
        let mut a2 = ActionState::default();
        a2.press(&Action::Jump);
        a1.action_data_mut(&Action::Jump).unwrap().value = 1.0;
        input_buffer.set(Tick(3), &a1);
        input_buffer.set(Tick(6), &a2);
        input_buffer.set(Tick(7), &a2);

        assert_eq!(input_buffer.start_tick, Some(Tick(3)));
        assert_eq!(input_buffer.buffer.len(), 5);

        assert_eq!(input_buffer.get(Tick(3)), Some(&a1));
        assert_eq!(input_buffer.get(Tick(4)), Some(&a1));
        assert_eq!(input_buffer.get(Tick(5)), Some(&a1));
        assert_eq!(input_buffer.get(Tick(6)), Some(&a2));
        assert_eq!(input_buffer.get(Tick(8)), None);

        assert_eq!(input_buffer.pop(Tick(4)), Some(a1.clone()));
        assert_eq!(input_buffer.start_tick, Some(Tick(5)));
        assert_eq!(input_buffer.buffer.len(), 3);

        // the oldest element has been updated from `SameAsPrecedent` to `Data`
        assert_eq!(
            input_buffer.buffer.front().unwrap(),
            &BufferItem::Data(a1.clone())
        );
        assert_eq!(input_buffer.pop(Tick(7)), Some(a2.clone()));
        assert_eq!(input_buffer.start_tick, Some(Tick(8)));
        assert_eq!(input_buffer.buffer.len(), 0);
    }
}
