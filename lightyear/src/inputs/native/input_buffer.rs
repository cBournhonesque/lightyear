use super::{ActionState, UserAction};
use crate::shared::tick_manager::Tick;
use alloc::collections::VecDeque;
#[cfg(not(feature = "std"))]
use alloc::{format, string::{String, ToString}, vec::Vec};
use bevy::prelude::Component;
use core::fmt::{Debug, Formatter};
use serde::{Deserialize, Serialize};
use tracing::trace;

#[derive(Component, Debug)]
pub struct InputBuffer<T> {
    pub(crate) start_tick: Option<Tick>,
    pub(crate) buffer: VecDeque<InputData<T>>,
}

impl<T: Debug> core::fmt::Display for InputBuffer<T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> core::fmt::Result {
        let ty = core::any::type_name::<T>();

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
        match value { Some(value) => {
            InputData::Input(value)
        } _ => {
            InputData::Absent
        }}
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
    pub(crate) fn update_from_message(&mut self, end_tick: Tick, values: &Vec<InputData<T>>) {
        let start_tick = end_tick + 1 - values.len() as u16;
        // the first value is guaranteed to not be SameAsPrecedent
        for (delta, input) in values.iter().enumerate() {
            let tick = start_tick + Tick(delta as u16);
            match input {
                InputData::Absent => {
                    self.set_raw(tick, InputData::Input(ActionState::<T> { value: None }));
                }
                InputData::SameAsPrecedent => {
                    self.set_raw(tick, InputData::SameAsPrecedent);
                }
                InputData::Input(input) => {
                    // do not set the value if it's equal to what's already in the buffer
                    if self.get(tick).is_some_and(|existing_value| {
                        existing_value.value.as_ref().is_some_and(|v| v == input)
                    }) {
                        continue;
                    }
                    self.set(
                        tick,
                        ActionState::<T> {
                            value: Some(input.clone()),
                        },
                    );
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
        if let Some(precedent) = self.get(tick - 1) {
            if precedent == &value {
                self.set_raw(tick, InputData::SameAsPrecedent);
                return;
            }
        }
        self.set_raw(tick, InputData::Input(value));
    }

    // Note: we expect this to be set every tick?
    //  i.e. there should be an ActionState for every tick, even if the action is None
    /// Set the ActionState for the given tick in the InputBuffer
    ///
    /// This should be called every tick.
    pub fn set_empty(&mut self, tick: Tick) {
        self.set_raw(tick, InputData::Absent);
    }

    pub(crate) fn set_raw(&mut self, tick: Tick, value: InputData<T>) {
        let Some(start_tick) = self.start_tick else {
            // initialize the buffer
            self.start_tick = Some(tick);
            self.buffer.push_back(value);
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

        // safety: we are guaranteed that the tick is in the buffer
        let entry = self.buffer.get_mut((tick - start_tick) as usize).unwrap();
        *entry = value;
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

        match popped { InputData::Input(value) => {
            Some(value)
        } _ => {
            None
        }}
    }

    pub(crate) fn get_raw(&self, tick: Tick) -> &InputData<T> {
        let Some(start_tick) = self.start_tick else {
            return &InputData::Absent;
        };
        if self.buffer.is_empty() {
            return &InputData::Absent;
        }
        if tick < start_tick || tick > start_tick + (self.buffer.len() as i16 - 1) {
            return &InputData::Absent;
        }
        self.buffer.get((tick - start_tick) as usize).unwrap()
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
        input_buffer.set(Tick(8), 1);

        assert_eq!(input_buffer.get(Tick(4)), Some(&0));
        // missing ticks are filled with SameAsPrecedent
        assert_eq!(input_buffer.get(Tick(5)), Some(&0));
        assert_eq!(input_buffer.get_raw(Tick(5)), &InputData::SameAsPrecedent);
        assert_eq!(input_buffer.get(Tick(6)), Some(&1));
        // similar values are compressed
        assert_eq!(input_buffer.get_raw(Tick(7)), &InputData::SameAsPrecedent);
        assert_eq!(input_buffer.get_raw(Tick(8)), &InputData::SameAsPrecedent);
        // we get None if we try to get a value outside the buffer
        assert_eq!(input_buffer.get(Tick(9)), None);

        // we get the correct value even if we pop SameAsPrecedent
        assert_eq!(input_buffer.pop(Tick(5)), Some(0));
        assert_eq!(input_buffer.start_tick, Some(Tick(6)));

        // if the next value in the buffer after we pop is SameAsPrecedent, it should
        // get replaced with a real value
        assert_eq!(input_buffer.pop(Tick(7)), Some(1));
        assert_eq!(input_buffer.start_tick, Some(Tick(8)));
        assert_eq!(input_buffer.get(Tick(8)), Some(&1));
        assert_eq!(input_buffer.get_raw(Tick(8)), &InputData::Input(1));
        assert_eq!(input_buffer.buffer.len(), 1);
    }
}
