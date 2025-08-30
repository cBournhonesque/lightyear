//! The InputBuffer contains a history of the ActionState for each tick.
//!
//! It is used for several purposes:
//! - the client's inputs for tick T must arrive before the server processes tick T, so they are stored
//!   in the buffer until the server processes them. The InputBuffer can be updated efficiently by receiving
//!   a list of `ActionDiff`s compared from an initial `ActionState`
//! - to implement input-delay, we want a button press at tick t to be processed at tick t + delay on the client.
//!   Therefore, we will store the computed ActionState at tick t + delay, but then we load the ActionState at tick t
//!   from the buffer
use alloc::collections::VecDeque;
use alloc::{
    format,
    string::{String, ToString},
    vec::Vec,
};
use bevy_ecs::component::Component;
use bevy_reflect::Reflect;
use core::fmt::{Debug, Formatter};
use lightyear_core::tick::Tick;
use serde::{Deserialize, Serialize};
use tracing::trace;

#[derive(Component, Debug, Reflect)]
pub struct InputBuffer<T> {
    pub start_tick: Option<Tick>,
    pub buffer: VecDeque<InputData<T>>,
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
                    InputData::Input(data) => format!("{data:?}"),
                };
                format!("{:?}: {}\n", tick + i as i16, str)
            })
            .collect::<Vec<String>>()
            .join("");
        write!(f, "InputBuffer<{ty:?}>:\n {buffer_str}")
    }
}

/// We use this structure to efficiently compress the inputs that we send to the server
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug, Reflect)]
pub enum InputData<T> {
    Absent,
    SameAsPrecedent,
    Input(T),
}

impl<T> From<Option<T>> for InputData<T> {
    fn from(value: Option<T>) -> Self {
        match value {
            Some(value) => InputData::Input(value),
            _ => InputData::Absent,
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

impl<T: Clone + PartialEq> InputBuffer<T> {
    /// Number of elements in the buffer
    pub(crate) fn len(&self) -> usize {
        self.buffer.len()
    }

    /// Make sure that the buffer fits the range [start_tick, end_tick]
    ///
    /// This is used when we receive a new InputMessage, to update the buffer from the message.
    /// It is important to extend the range, otherwise `get_raw` might return immediately if the tick is outside the current range.
    pub fn extend_to_range(&mut self, start_tick: Tick, end_tick: Tick) {
        if self.start_tick.is_none() {
            self.start_tick = Some(start_tick);
        }
        let mut current_start = self.start_tick.unwrap();
        // Extend to the left if needed
        if start_tick < current_start {
            let prepend_count = (current_start - start_tick) as usize;
            for _ in 0..prepend_count {
                self.buffer.push_front(InputData::Absent);
            }
            self.start_tick = Some(start_tick);
            current_start = start_tick;
        }

        // Extend to the right if needed
        let current_end = current_start + (self.buffer.len() as i16 - 1);
        if end_tick > current_end {
            let append_count = (end_tick - current_end) as usize;
            for _ in 0..append_count {
                self.buffer.push_back(InputData::Absent);
            }
        }
    }

    // Note: we expect this to be set every tick?
    //  i.e. there should be an ActionState for every tick, even if the action is None
    /// Set the ActionState for the given tick in the InputBuffer
    ///
    /// This should be called every tick.
    pub fn set(&mut self, tick: Tick, value: T) {
        if let Some(precedent) = self.get(tick - 1)
            && precedent == &value
        {
            self.set_raw(tick, InputData::SameAsPrecedent);
            return;
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

    pub fn set_raw(&mut self, tick: Tick, value: InputData<T>) {
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

    /// Remove all the inputs that are older or equal than the given tick, then return the input
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
            let data = self.buffer.pop_front().unwrap();
            match data {
                InputData::Absent | InputData::Input(_) => {
                    popped = data;
                }
                _ => {}
            }
        }
        self.start_tick = Some(tick + 1);

        // if the next value after we popped was 'SameAsPrecedent', we need to override it with an actual value
        if let Some(InputData::SameAsPrecedent) = self.buffer.front() {
            *self.buffer.front_mut().unwrap() = popped.clone();
        }

        match popped {
            InputData::Input(value) => Some(value),
            _ => None,
        }
    }

    /// Get the raw `InputData` for the given tick, without resolving `SameAsPrecedent`
    pub fn get_raw(&self, tick: Tick) -> &InputData<T> {
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

    /// Get the `ActionState` for the given tick. This does not apply prediction:
    /// - if the tick is outside the range of the buffer, it returns None
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

    /// Get the `ActionState` for the given tick.
    /// This applies prediction:
    /// - if the tick is outside the range of the buffer, we return the last known ActionState (if any)
    pub fn get_predict(&self, tick: Tick) -> Option<&T> {
        let start_tick = self.start_tick?;
        if self.buffer.is_empty() {
            return None;
        }
        if tick < start_tick {
            return None;
        }
        if tick > start_tick + (self.buffer.len() as i16 - 1) {
            return self.get_last();
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
    #[inline(always)]
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

    #[test]
    fn test_extend_to_range_empty() {
        let mut input_buffer: InputBuffer<i32> = InputBuffer::default();
        input_buffer.extend_to_range(Tick(5), Tick(7));
        assert_eq!(input_buffer.start_tick, Some(Tick(5)));
        assert_eq!(input_buffer.buffer.len(), 3);
        assert_eq!(input_buffer.get_raw(Tick(5)), &InputData::Absent);
        assert_eq!(input_buffer.get_raw(Tick(6)), &InputData::Absent);
        assert_eq!(input_buffer.get_raw(Tick(7)), &InputData::Absent);
    }

    #[test]
    fn test_extend_to_range_right() {
        let mut input_buffer: InputBuffer<i32> = InputBuffer::default();
        input_buffer.set(Tick(10), 42);
        input_buffer.extend_to_range(Tick(10), Tick(13));
        assert_eq!(input_buffer.start_tick, Some(Tick(10)));
        assert_eq!(input_buffer.buffer.len(), 4);
        assert_eq!(input_buffer.get_raw(Tick(10)), &InputData::Input(42));
        assert_eq!(input_buffer.get_raw(Tick(11)), &InputData::Absent);
        assert_eq!(input_buffer.get_raw(Tick(12)), &InputData::Absent);
        assert_eq!(input_buffer.get_raw(Tick(13)), &InputData::Absent);
    }

    #[test]
    fn test_extend_to_range_left() {
        let mut input_buffer: InputBuffer<i32> = InputBuffer::default();
        input_buffer.set(Tick(10), 42);
        input_buffer.extend_to_range(Tick(8), Tick(10));
        assert_eq!(input_buffer.start_tick, Some(Tick(8)));
        assert_eq!(input_buffer.buffer.len(), 3);
        assert_eq!(input_buffer.get_raw(Tick(8)), &InputData::Absent);
        assert_eq!(input_buffer.get_raw(Tick(9)), &InputData::Absent);
        assert_eq!(input_buffer.get_raw(Tick(10)), &InputData::Input(42));
    }

    #[test]
    fn test_extend_to_range_both_sides() {
        let mut input_buffer: InputBuffer<i32> = InputBuffer::default();
        input_buffer.set(Tick(5), 1);
        input_buffer.set(Tick(6), 2);
        input_buffer.extend_to_range(Tick(3), Tick(8));
        assert_eq!(input_buffer.start_tick, Some(Tick(3)));
        assert_eq!(input_buffer.buffer.len(), 6);
        assert_eq!(input_buffer.get_raw(Tick(3)), &InputData::Absent);
        assert_eq!(input_buffer.get_raw(Tick(4)), &InputData::Absent);
        assert_eq!(input_buffer.get_raw(Tick(5)), &InputData::Input(1));
        assert_eq!(input_buffer.get_raw(Tick(6)), &InputData::Input(2));
        assert_eq!(input_buffer.get_raw(Tick(7)), &InputData::Absent);
        assert_eq!(input_buffer.get_raw(Tick(8)), &InputData::Absent);
    }

    #[test]
    fn test_set_empty_and_get_raw() {
        let mut input_buffer: InputBuffer<i32> = InputBuffer::default();
        input_buffer.set_empty(Tick(3));
        assert_eq!(input_buffer.get_raw(Tick(3)), &InputData::Absent);
        assert_eq!(input_buffer.get(Tick(3)), None);
    }

    #[test]
    fn test_set_raw_and_get() {
        let mut input_buffer: InputBuffer<i32> = InputBuffer::default();
        input_buffer.set_raw(Tick(2), InputData::Input(7));
        assert_eq!(input_buffer.get(Tick(2)), Some(&7));
        input_buffer.set_raw(Tick(3), InputData::SameAsPrecedent);
        assert_eq!(input_buffer.get(Tick(3)), Some(&7));
    }

    #[test]
    fn test_get_last_and_get_last_with_tick() {
        let mut input_buffer: InputBuffer<i32> = InputBuffer::default();
        assert_eq!(input_buffer.get_last(), None);
        assert_eq!(input_buffer.get_last_with_tick(), None);

        input_buffer.set(Tick(1), 10);
        input_buffer.set(Tick(2), 20);
        assert_eq!(input_buffer.get_last(), Some(&20));
        assert_eq!(input_buffer.get_last_with_tick(), Some((Tick(2), &20)));
    }

    #[test]
    fn test_end_tick() {
        let mut input_buffer: InputBuffer<i32> = InputBuffer::default();
        assert_eq!(input_buffer.end_tick(), None);
        input_buffer.set(Tick(5), 1);
        assert_eq!(input_buffer.end_tick(), Some(Tick(5)));
        input_buffer.set(Tick(7), 2);
        assert_eq!(input_buffer.end_tick(), Some(Tick(7)));
    }

    #[test]
    fn test_pop_with_absent() {
        let mut input_buffer: InputBuffer<i32> = InputBuffer::default();
        input_buffer.set(Tick(1), 1);
        input_buffer.set(Tick(2), 2);
        input_buffer.set_empty(Tick(3));
        input_buffer.set(Tick(4), 2);
        // Pop up to tick 2
        assert_eq!(input_buffer.pop(Tick(2)), Some(2));
        // Now tick 3 is Absent, so pop returns None
        assert_eq!(input_buffer.pop(Tick(3)), None);
        // Now tick 4 is Input(2)
        assert_eq!(input_buffer.pop(Tick(4)), Some(2));
    }

    #[test]
    fn test_pop_out_of_range() {
        let mut input_buffer: InputBuffer<i32> = InputBuffer::default();
        input_buffer.set(Tick(10), 5);
        // Pop before start_tick
        assert_eq!(input_buffer.pop(Tick(5)), None);
        // Pop after end_tick
        assert_eq!(input_buffer.pop(Tick(20)), None);
        assert_eq!(input_buffer.buffer.len(), 0);
        assert_eq!(input_buffer.start_tick, Some(Tick(21)));
    }

    #[test]
    fn test_pop_same_absent_in_gap() {
        let mut input_buffer: InputBuffer<i32> = InputBuffer::default();
        input_buffer.set(Tick(9), 5);
        input_buffer.set(Tick(10), 5);
        input_buffer.set_empty(Tick(11));
        input_buffer.set_empty(Tick(12));
        input_buffer.set_empty(Tick(13));
        // Pop before start_tick
        assert_eq!(input_buffer.pop(Tick(12)), None);
        assert_eq!(input_buffer.get(Tick(13)), None);
        assert_eq!(input_buffer.buffer.len(), 1);
    }

    #[test]
    fn test_len() {
        let mut input_buffer: InputBuffer<i32> = InputBuffer::default();
        assert_eq!(input_buffer.len(), 0);
        input_buffer.set(Tick(1), 1);
        assert_eq!(input_buffer.len(), 1);
        input_buffer.set(Tick(2), 2);
        assert_eq!(input_buffer.len(), 2);
    }
}
