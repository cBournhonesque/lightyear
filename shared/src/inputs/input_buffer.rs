use crate::tick::Tick;
use crate::{BitSerializable, ReadBuffer, ReadyBuffer, SequenceBuffer, WriteBuffer};
use bevy::prelude::{In, Resource};
use bitcode::{Decode, Encode};
use lightyear_derive::{Message, MessageInternal};
use serde::{Deserialize, Serialize};

// TODO: should we request that a user input is a message?
pub trait UserInput: BitSerializable + Clone + Eq + PartialEq + Send + Sync + 'static {}
impl UserInput for () {}

// OPTION 1: could do something similar to the prediction history (ready buffer and we don't include the gaps).
//  but seems less optimized and overly complicated

// OPTION 2: use a ringbuffer?

// this should be more than enough, maybe make smaller or tune depending on latency?
const INPUT_BUFFER_SIZE: usize = 128;

#[derive(Resource)]
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

#[derive(MessageInternal, Serialize, Deserialize, Clone, PartialEq, Debug)]
/// Message that we use to send the client inputs to the server
/// We will store the last N inputs starting from start_tick (in case of packet loss)
pub struct InputMessage<T: UserInput> {
    end_tick: Tick,
    // first element is tick end_tick-N+1, last element is end_tick
    inputs: Vec<InputData<T>>,
    // inputs: [InputData<T>; N],
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
        let message_start_tick = Tick(message.end_tick.0 - message.inputs.len() as u16 + 1);

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
                    // guaranteed to exist because we just pushed it, and because of how InputMessage is constructed
                    let prev_value = self.buffer.get(&prev_tick).unwrap();
                    self.buffer.push(&tick, prev_value.clone());
                }
                InputData::Input(input) => {
                    self.buffer.push(&tick, input.clone());
                }
            }
        }
    }

    // Convert the last N ticks up to end_tick included into a compressed message that we can send to the server
    pub(crate) fn create_message(&self, end_tick: Tick, num_ticks: u16) -> InputMessage<T> {
        let mut inputs = Vec::new();
        let mut current_tick = end_tick;
        // start with the first value
        let start_tick = Tick(end_tick.0 - num_ticks + 1);
        inputs.push(
            self.buffer
                .get(&start_tick)
                .map_or(InputData::Absent, |input| InputData::Input(input.clone())),
        );

        for delta in 1..num_ticks {
            let tick = start_tick + Tick(delta);
            // safe because we keep pushing elements
            let prev_value = inputs.last().unwrap();
            let value = self
                .buffer
                .get(&tick)
                .map_or(InputData::Absent, |input| InputData::Input(input.clone()));
            inputs.push(if prev_value == &value {
                InputData::SameAsPrecedent
            } else {
                value
            });
        }
        InputMessage { inputs, end_tick }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    impl UserInput for usize {}

    #[test]
    fn test_input_buffer() {
        let mut input_buffer = InputBuffer::default();

        input_buffer.buffer.push(&Tick(4), 0);
        input_buffer.buffer.push(&Tick(6), 1);
        input_buffer.buffer.push(&Tick(7), 1);

        let message = input_buffer.create_message(Tick(9), 7);
        assert_eq!(
            message,
            InputMessage {
                end_tick: Tick(9),
                inputs: vec![
                    InputData::Absent,
                    InputData::Input(0),
                    InputData::Absent,
                    InputData::Input(1),
                    InputData::SameAsPrecedent,
                    InputData::Absent,
                    InputData::SameAsPrecedent,
                ],
            }
        );
    }
}
