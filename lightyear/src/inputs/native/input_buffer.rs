use super::{ActionState, UserAction};
use crate::shared::tick_manager::Tick;
use bevy::prelude::{Component, Resource};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::fmt::{Debug, Formatter};
use std::time::Instant;
use steamworks::Input;
use tracing::trace;

#[derive(Component, Debug)]
pub struct InputBuffer<T> {
    pub(crate) start_tick: Option<Tick>,
    pub(crate) buffer: VecDeque<InputData<T>>,
}

impl<T: Debug> std::fmt::Display for InputBuffer<T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let ty = std::any::type_name::<T>();

        let Some(tick) = self.start_tick else {
            return write!(f, "EmptyInputBuffer");
        };

        let buffer_str = self
            .buffer
            .iter()
            .enumerate()
            .map(|(i, item)| {
                let str = match item {
                    InputData::Absent => "Absent".to_string(),
                    InputData::SameAsPrecedent => "SameAsPrecedent".to_string(),
                    InputData::Input(data) => format!("{:?}", data),
                };
                format!("{:?}: {}\n", tick + i as i16, str)
            })
            .collect::<Vec<String>>()
            .join("");
        write!(f, "InputBuffer<{:?}>:\n {}", ty, buffer_str)
    }
}


/// We use this structure to efficiently compress the inputs that we send to the server
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub(crate) enum InputData<T> {
    Absent,
    SameAsPrecedent,
    Input(T),
}

impl<T> From<Option<T>> for InputData<T> {
    fn from(value: Option<T>) -> Self {
        if let Some(value) = value {
            InputData::Input(value)
        } else {
            InputData::Absent
        }
    }
}

impl<T> Default for InputBuffer<T> {
    fn default() -> Self {
        Self {
            buffer: VecDeque::new(),
            start_tick: None,
        }
    }
}

impl<T: UserAction> InputBuffer<ActionState<T>> {
        /// Upon receiving an [`InputMessage`](super::input_message::InputMessage), update the InputBuffer with all the inputs
    /// included in the message.
    /// TODO: disallow overwriting inputs for ticks we've already received inputs for?
    ///
    pub(crate) fn update_from_message(
        &mut self,
        end_tick: Tick,
        values: &Vec<InputData<T>>,
    ) {
        let start_tick = end_tick - values.len() as u16;
        let mut precedent = ActionState::<T>::default();
        // the first value is guaranteed to not be SameAsPrecedent
        for (delta, input) in values.iter().enumerate() {
            let tick = start_tick + Tick(delta as u16);
            match input {
                InputData::Absent => {
                    self.set_empty(tick);
                    precedent = ActionState::<T>::default();
                }
                InputData::SameAsPrecedent => {
                    // TODO: we can directly write 'SameAsPrecedent' in the buffer!

                    if let Some(input) = &precedent.value {
                         // do not set the value if it's equal to what's already in the buffer
                        if self
                            .get(tick)
                            .is_some_and(|existing_value| existing_value.value.as_ref().is_some_and(|v| v == input))
                        {
                            continue;
                        }
                        self.set(tick, input.clone());
                    } else {
                        self.set_empty(tick);
                    }
                }
                InputData::Input(input) => {
                    precedent = input.clone();
                    // do not set the value if it's equal to what's already in the buffer
                    if self
                        .get(tick)
                        .is_some_and(|existing_value| existing_value.value.as_ref().is_some_and(|v| v == input))
                    {
                        continue;
                    }
                    self.set(tick, input.clone());
                }
            }
        }
    }
}

impl<T: Clone + PartialEq> InputBuffer<T> {
    /// Number of elements in the buffer
    pub(crate) fn len(&self) -> usize {
        self.buffer.len()
    }

    // Note: we expect this to be set every tick?
    //  i.e. there should be an ActionState for every tick, even if the action is None
    /// Set the ActionState for the given tick in the InputBuffer
    ///
    /// This should be called every tick.
    pub fn set(&mut self, tick: Tick, value: T) {
        let Some(start_tick) = self.start_tick else {
            // initialize the buffer
            self.start_tick = Some(tick);
            self.buffer.push_back(InputData::Input(value));
            return;
        };

        // cannot set lower values than start_tick
        if tick < start_tick {
            return;
        }

        let end_tick = start_tick + (self.buffer.len() as i16 - 1);

        // NOTE: we fill the value for the given tick, and we fill the ticks between start_tick and tick
        // with InputData::SameAsPrecedent (i.e. if there are any gaps, we consider that the user repeated
        // their last action)
        if tick > end_tick {
            // TODO: Think about how to fill the buffer between ticks
            //  - we want: if an input is missing, we consider that the user did the same action (RocketLeague or Overwatch GDC)

            // TODO: think about whether this is correct or not, it is correct if we always call set()
            //  with monotonically increasing ticks, which I think is the case
            //  maybe that's not correct because the timing information should be different? (i.e. I should tick the action-states myself and set them)
            // fill the ticks between end_tick and tick with a copy of the current ActionState
            for _ in 0..(tick - end_tick - 1) {
                trace!("fill ticks");
                self.buffer.push_back(InputData::SameAsPrecedent);
            }
            // add a new value to the buffer, which we will override below
            self.buffer.push_back(InputData::Absent);
        }

        // check if the value is the same as the precedent tick, in which case we compress it
        let mut same_as_precedent = false;
        if let Some(action_state) = self.get(end_tick) {
            if action_state == &value {
                same_as_precedent = true;
            }
        }

        // safety: we are guaranteed that the tick is in the buffer
        let entry = self.buffer.get_mut((tick - start_tick) as usize).unwrap();
        if same_as_precedent {
            *entry = InputData::SameAsPrecedent;
        } else {
            *entry = InputData::Input(value);
        }
    }

    // Note: we expect this to be set every tick?
    //  i.e. there should be an ActionState for every tick, even if the action is None
    /// Set the ActionState for the given tick in the InputBuffer
    ///
    /// This should be called every tick.
    pub fn set_empty(&mut self, tick: Tick) {
        let Some(start_tick) = self.start_tick else {
            // initialize the buffer
            self.start_tick = Some(tick);
            self.buffer.push_back(InputData::Absent);
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
                trace!("fill ticks");
                self.buffer.push_back(InputData::Absent);
            }
            // add a new value to the buffer, which we will override below
            self.buffer.push_back(InputData::Absent);
        }

        // safety: we are guaranteed that the tick is in the buffer
        let entry = self.buffer.get_mut((tick - start_tick) as usize).unwrap();
        *entry = InputData::Absent;
    }

    /// Remove all the inputs that are older than the given tick, then return the input
    /// for the given tick
    pub fn pop(&mut self, tick: Tick) -> Option<T> {
        let start_tick = self.start_tick?;
        if tick < start_tick {
            return None;
        }
        if tick > start_tick + (self.buffer.len() as i16 - 1) {
            // pop everything
            self.buffer = VecDeque::new();
            self.start_tick = Some(tick + 1);
            return None;
        }

        // popped will represent the last value popped
        let mut popped = InputData::Absent;
        for _ in 0..(tick + 1 - start_tick) {
            // front is the oldest value
            let data = self.buffer.pop_front();
            if let Some(InputData::Input(value)) = data {
                popped = InputData::Input(value);
            }
        }
        self.start_tick = Some(tick + 1);

        // if the next value after we popped was 'SameAsPrecedent', we need to override it with an actual value
        if let Some(InputData::SameAsPrecedent) = self.buffer.front() {
            *self.buffer.front_mut().unwrap() = popped.clone();
        }

        if let InputData::Input(value) = popped {
            Some(value)
        } else {
            None
        }
    }

    /// Get the [`ActionState`] for the given tick
    pub fn get(&self, tick: Tick) -> Option<&T> {
        let start_tick = self.start_tick?;
        if self.buffer.is_empty() {
            return None;
        }
        if tick < start_tick || tick > start_tick + (self.buffer.len() as i16 - 1) {
            return None;
        }
        let data = self.buffer.get((tick - start_tick) as usize).unwrap();
        match data {
            InputData::Absent => None,
            InputData::SameAsPrecedent => {
                // get the data from the preceding tick
                self.get(tick - 1)
            }
            InputData::Input(data) => Some(data),
        }
    }

    /// Get latest ActionState present in the buffer
    pub fn get_last(&self) -> Option<&T> {
        let start_tick = self.start_tick?;
        if self.buffer.is_empty() {
            return None;
        }
        self.get(start_tick + (self.buffer.len() as i16 - 1))
    }

    /// Get latest ActionState present in the buffer, along with the associated Tick
    pub fn get_last_with_tick(&self) -> Option<(Tick, &T)> {
        let start_tick = self.start_tick?;
        if self.buffer.is_empty() {
            return None;
        }
        let end_tick = start_tick + (self.buffer.len() as i16 - 1);
        self.get(end_tick)
            .map(|action_state| (end_tick, action_state))
    }

    /// Get the last tick in the buffer
    pub fn end_tick(&self) -> Option<Tick> {
        self.start_tick
            .map(|start_tick| start_tick + (self.buffer.len() as i16 - 1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_set_pop() {
        let mut input_buffer = InputBuffer::default();

        input_buffer.set(Tick(4), 0);
        input_buffer.set(Tick(6), 1);
        input_buffer.set(Tick(7), 1);

        assert_eq!(input_buffer.get(Tick(4)), Some(&0));
        assert_eq!(input_buffer.get(Tick(5)), Some(&0));
        assert_eq!(input_buffer.get(Tick(6)), Some(&1));
        assert_eq!(input_buffer.get(Tick(8)), None);

        assert_eq!(input_buffer.pop(Tick(5)), Some(0));
        assert_eq!(input_buffer.start_tick, Some(Tick(6)));
        assert_eq!(input_buffer.pop(Tick(7)), Some(1));
        assert_eq!(input_buffer.start_tick, Some(Tick(8)));
        assert_eq!(input_buffer.buffer.len(), 0);
    }
}
