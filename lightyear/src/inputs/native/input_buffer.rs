use std::collections::VecDeque;
use std::fmt::Debug;

use bevy::prelude::Resource;
use serde::{Deserialize, Serialize};
use tracing::{info, trace};

use lightyear_macros::MessageInternal;

use crate::protocol::BitSerializable;
use crate::shared::tick_manager::Tick;

use super::UserAction;

#[derive(Resource, Debug)]
pub struct InputBuffer<T: UserAction> {
    pub buffer: VecDeque<Option<T>>,
    pub start_tick: Option<Tick>,
}

// TODO: add encode directive to encode even more efficiently
/// We use this structure to efficiently compress the inputs that we send to the server
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub(crate) enum InputData<T: UserAction> {
    Absent,
    SameAsPrecedent,
    Input(T),
}

// TODO: use Mode to specify how to serialize a message (serde vs bitcode)! + can specify custom serialize function as well (similar to interpolation mode)
#[derive(MessageInternal, Serialize, Deserialize, Clone, PartialEq, Debug)]
/// Message that we use to send the client inputs to the server
/// We will store the last N inputs starting from start_tick (in case of packet loss)
pub struct InputMessage<T: UserAction> {
    pub(crate) end_tick: Tick,
    // first element is tick end_tick-N+1, last element is end_tick
    pub(crate) inputs: Vec<InputData<T>>,
}

impl<T: UserAction> InputMessage<T> {
    pub fn is_empty(&self) -> bool {
        if self.inputs.len() == 0 {
            return true;
        }
        let mut iter = self.inputs.iter();
        if iter.next().unwrap() == &InputData::Absent {
            return iter.all(|x| x == &InputData::SameAsPrecedent);
        }
        false
    }
}

impl<T: UserAction> Default for InputBuffer<T> {
    fn default() -> Self {
        Self {
            // buffer: SequenceBuffer::new(),
            buffer: VecDeque::new(),
            start_tick: None,
            // end_tick: Tick(0),
        }
    }
}

impl<T: UserAction> InputBuffer<T> {
    // pub(crate) fn remove(&mut self, tick: Tick) -> Option<T> {
    //     if tick < self.start_tick || tick > self.end_tick {
    //         return None;
    //     }
    //     self.buffer.remove(&tick)
    // }

    /// Remove all the inputs that are older than the given tick, then return the input
    /// for the given tick
    pub(crate) fn pop(&mut self, tick: Tick) -> Option<T> {
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
        // info!(
        //     "buffer: {:?}. start_tick: {:?}, tick: {:?}",
        //     self.buffer, self.start_tick, tick
        // );
        for _ in 0..(tick - start_tick) {
            self.buffer.pop_front();
        }
        self.start_tick = Some(tick + 1);
        self.buffer.pop_front().unwrap()
    }

    pub(crate) fn get(&self, tick: Tick) -> Option<&T> {
        let start_tick = self.start_tick?;
        if self.buffer.is_empty() {
            return None;
        }
        if tick < start_tick || tick > start_tick + (self.buffer.len() as i16 - 1) {
            return None;
        }
        self.buffer
            .get((tick - start_tick) as usize)
            .unwrap()
            .as_ref()
    }

    pub(crate) fn set(&mut self, tick: Tick, value: Option<T>) {
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
        if tick > end_tick {
            for _ in 0..(tick - end_tick - 1) {
                self.buffer.push_back(None);
            }
            self.buffer.push_back(value);
            return;
        }
        // safety: we are guaranteed that the tick is in the buffer
        *self.buffer.get_mut((tick - start_tick) as usize).unwrap() = value;
    }

    /// We received a new input message from the user, and use it to update the input buffer
    /// TODO: should we keep track of which inputs in the input buffer are absent and only update those?
    ///  The current tick is the current server tick, no need to update the buffer for ticks that are older than that
    pub(crate) fn update_from_message(&mut self, message: InputMessage<T>) {
        let message_start_tick = Tick(message.end_tick.0) - message.inputs.len() as u16 + 1;
        let mut prev_value = None;

        for (delta, input) in message.inputs.into_iter().enumerate() {
            let tick = message_start_tick + Tick(delta as u16);
            match input {
                InputData::Absent => {
                    prev_value = None;
                    self.set(tick, None);
                }
                InputData::SameAsPrecedent => {
                    self.set(tick, prev_value.clone());
                }
                InputData::Input(input) => {
                    prev_value = Some(input);
                    if self.get(tick) == prev_value.as_ref() {
                        continue;
                    } else {
                        self.set(tick, prev_value.clone());
                    }
                }
            }
        }
    }

    // Convert the last N ticks up to end_tick included into a compressed message that we can send to the server
    // Return None if the last N inputs are all Absent
    pub(crate) fn create_message(&self, end_tick: Tick, num_ticks: u16) -> InputMessage<T> {
        let mut inputs = Vec::new();
        // start with the first value
        let start_tick = Tick(end_tick.0) - num_ticks + 1;
        inputs.push(
            self.get(start_tick)
                .map_or(InputData::Absent, |input| InputData::Input(input.clone())),
        );
        // keep track of the previous value to avoid sending the same value multiple times
        let mut prev_value_idx = 0;
        for delta in 1..num_ticks {
            let tick = start_tick + Tick(delta);
            // safe because we keep pushing elements
            let value = self
                .get(tick)
                .map_or(InputData::Absent, |input| InputData::Input(input.clone()));
            // safe before prev_value_idx is always present
            if inputs.get(prev_value_idx).unwrap() == &value {
                inputs.push(InputData::SameAsPrecedent);
            } else {
                prev_value_idx = inputs.len();
                inputs.push(value);
            }
        }
        InputMessage { inputs, end_tick }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    impl UserAction for usize {}

    #[test]
    fn test_get_set_pop() {
        let mut input_buffer = InputBuffer::default();

        input_buffer.set(Tick(4), Some(0));
        input_buffer.set(Tick(6), Some(1));
        input_buffer.set(Tick(7), Some(1));

        assert_eq!(input_buffer.get(Tick(4)), Some(&0));
        assert_eq!(input_buffer.get(Tick(5)), None);
        assert_eq!(input_buffer.get(Tick(6)), Some(&1));
        assert_eq!(input_buffer.get(Tick(8)), None);

        assert_eq!(input_buffer.pop(Tick(5)), None);
        assert_eq!(input_buffer.start_tick, Some(Tick(6)));
        assert_eq!(input_buffer.pop(Tick(7)), Some(1));
        assert_eq!(input_buffer.start_tick, Some(Tick(8)));
        assert_eq!(input_buffer.buffer.len(), 0);
    }

    #[test]
    fn test_create_message() {
        let mut input_buffer = InputBuffer::default();

        input_buffer.set(Tick(4), Some(0));
        input_buffer.set(Tick(6), Some(1));
        input_buffer.set(Tick(7), Some(1));

        let message = input_buffer.create_message(Tick(10), 8);
        assert_eq!(
            message,
            InputMessage {
                end_tick: Tick(10),
                inputs: vec![
                    InputData::Absent,
                    InputData::Input(0),
                    InputData::Absent,
                    InputData::Input(1),
                    InputData::SameAsPrecedent,
                    InputData::Absent,
                    InputData::SameAsPrecedent,
                    InputData::SameAsPrecedent,
                ],
            }
        );
    }

    #[test]
    fn test_update_from_message() {
        let mut input_buffer = InputBuffer::default();

        let message = InputMessage {
            end_tick: Tick(20),
            inputs: vec![
                InputData::Absent,
                InputData::Input(0),
                InputData::Absent,
                InputData::Input(1),
                InputData::SameAsPrecedent,
                InputData::Absent,
                InputData::SameAsPrecedent,
                InputData::SameAsPrecedent,
            ],
        };
        input_buffer.update_from_message(message);

        assert_eq!(input_buffer.get(Tick(20)), None);
        assert_eq!(input_buffer.get(Tick(19)), None);
        assert_eq!(input_buffer.get(Tick(18)), None);
        assert_eq!(input_buffer.get(Tick(17)), Some(&1));
        assert_eq!(input_buffer.get(Tick(16)), Some(&1));
        assert_eq!(input_buffer.get(Tick(15)), None);
        assert_eq!(input_buffer.get(Tick(14)), Some(&0));
        assert_eq!(input_buffer.get(Tick(13)), None);
    }
}
