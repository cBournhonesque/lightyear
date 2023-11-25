use bevy::prelude::Resource;
use bitcode::{Decode, Encode};
use serde::{Deserialize, Serialize};
use std::fmt::Debug;

use lightyear_derive::MessageInternal;

use crate::tick::Tick;
use crate::{BitSerializable, ReadBuffer, SequenceBuffer, WriteBuffer};

// TODO: should we request that a user input is a message?
pub trait UserInput:
    BitSerializable + Clone + Eq + PartialEq + Send + Sync + Debug + 'static
{
}

impl UserInput for () {}

// OPTION 1: could do something similar to the prediction history (ready buffer and we don't include the gaps).
//  but seems less optimized and overly complicated

// OPTION 2: use a ringbuffer?

// this should be more than enough, maybe make smaller or tune depending on latency?
const INPUT_BUFFER_SIZE: usize = 128;

#[derive(Resource, Debug)]
pub struct InputBuffer<T: UserInput> {
    pub buffer: SequenceBuffer<Tick, T, INPUT_BUFFER_SIZE>,
    // TODO: maybe keep track of the start?
}

// TODO: add encode directive to encode even more efficiently
/// We use this structure to efficiently compress the inputs that we send to the server
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
enum InputData<T: UserInput> {
    Absent,
    SameAsPrecedent,
    Input(T),
}

// TODO: use Mode to specify how to serialize a message (serde vs bitcode)! + can specify custom serialize function as well (similar to interpolation mode)
#[derive(MessageInternal, Serialize, Deserialize, Clone, PartialEq, Debug)]
/// Message that we use to send the client inputs to the server
/// We will store the last N inputs starting from start_tick (in case of packet loss)
pub struct InputMessage<T: UserInput> {
    end_tick: Tick,
    // first element is tick end_tick-N+1, last element is end_tick
    inputs: Vec<InputData<T>>,
}

impl<T: UserInput> InputMessage<T> {
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

impl<T: UserInput> Default for InputBuffer<T> {
    fn default() -> Self {
        Self {
            buffer: SequenceBuffer::new(),
        }
    }
}

impl<T: UserInput> InputBuffer<T> {
    /// We received a new input message from the user, and use it to update the input buffer
    /// TODO: should we keep track of which inputs in the input buffer are absent and only update those?
    ///  The current tick is the current server tick, no need to update the buffer for ticks that are older than that
    pub(crate) fn update_from_message(&mut self, message: &InputMessage<T>) {
        let message_start_tick = Tick(message.end_tick.0) - message.inputs.len() as u16 + 1;

        // // the input message is too old, don't do anything
        // if current_tick > message.end_tick {
        //     return;
        // }
        // let start_tick = message_start_tick.max(current_tick);

        for (delta, input) in message.inputs.iter().enumerate() {
            let tick = message_start_tick + Tick(delta as u16);
            match input {
                InputData::Absent => {
                    self.buffer.remove(&tick);
                }
                InputData::SameAsPrecedent => {
                    let prev_tick = tick - 1;
                    let prev_value = self.buffer.get(&prev_tick).cloned();
                    if let Some(v) = prev_value.or_else(|| self.buffer.remove(&tick)) {
                        self.buffer.push(&tick, v);
                    }
                }
                InputData::Input(input) => {
                    self.buffer.push(&tick, input.clone());
                }
            }
        }
    }

    // Convert the last N ticks up to end_tick included into a compressed message that we can send to the server
    // Return None if the last N inputs are all Absent
    pub(crate) fn create_message(&self, end_tick: Tick, num_ticks: u16) -> InputMessage<T> {
        let mut inputs = Vec::new();
        let mut current_tick = end_tick;
        // start with the first value
        let start_tick = Tick(end_tick.0) - num_ticks + 1;
        inputs.push(
            self.buffer
                .get(&start_tick)
                .map_or(InputData::Absent, |input| InputData::Input(input.clone())),
        );
        // keep track of the previous value to avoid sending the same value multiple times
        let mut prev_value_idx = 0;
        for delta in 1..num_ticks {
            let tick = start_tick + Tick(delta);
            // safe because we keep pushing elements
            let value = self
                .buffer
                .get(&tick)
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

    impl UserInput for usize {}

    #[test]
    fn test_create_message() {
        let mut input_buffer = InputBuffer::default();

        input_buffer.buffer.push(&Tick(4), 0);
        input_buffer.buffer.push(&Tick(6), 1);
        input_buffer.buffer.push(&Tick(7), 1);

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
        input_buffer.update_from_message(&message);

        assert_eq!(input_buffer.buffer.get(&Tick(20)), None);
        assert_eq!(input_buffer.buffer.get(&Tick(19)), None);
        assert_eq!(input_buffer.buffer.get(&Tick(18)), None);
        assert_eq!(input_buffer.buffer.get(&Tick(17)), Some(&1));
        assert_eq!(input_buffer.buffer.get(&Tick(16)), Some(&1));
        assert_eq!(input_buffer.buffer.get(&Tick(15)), None);
        assert_eq!(input_buffer.buffer.get(&Tick(14)), Some(&0));
        assert_eq!(input_buffer.buffer.get(&Tick(13)), None);
    }
}
