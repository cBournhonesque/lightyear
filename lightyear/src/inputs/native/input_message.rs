use crate::inputs::leafwing::action_diff::ActionDiff;
use crate::inputs::native::input_buffer::{InputBuffer, InputData};
use crate::inputs::native::ActionState;
use crate::prelude::client::InterpolationDelay;
use crate::prelude::{Deserialize, Serialize, Tick, UserAction};
use bevy::prelude::{Entity, Reflect};
use leafwing_input_manager::Actionlike;
use std::cmp::max;
use std::fmt::Write;

// TODO: use Mode to specify how to serialize a message (serde vs bitcode)! + can specify custom serialize function as well (similar to interpolation mode)
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Reflect)]
/// Message that we use to send the client inputs to the server
/// We will store the last N inputs starting from start_tick (in case of packet loss)
pub struct InputMessage<T> {
    /// Interpolation delay of the client at the time the message is sent
    ///
    /// We don't need any extra redundancy for the InterpolationDelay so we'll just send the value at `end_tick`.
    pub(crate) interpolation_delay: Option<InterpolationDelay>,
    pub(crate) end_tick: Tick,
    // first element is tick end_tick-N+1, last element is end_tick
    pub(crate) inputs: Vec<PerTargetData<T>>,
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug, Reflect)]
pub enum InputTarget {
    /// the input is for a predicted or confirmed entity: on the client, the server's local entity is mapped to the client's confirmed entity
    Entity(Entity),
    /// the input is for a pre-predicted entity: on the server, the server's local entity is mapped to the client's pre-predicted entity
    PrePredictedEntity(Entity),
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Reflect)]
pub(crate) struct PerTargetData<A: Actionlike> {
    pub(crate) target: InputTarget,
    // ActionState<A> from ticks `end_ticks-N` to `end_tick` (included)
    pub(crate) states: Vec<InputData<A>>,
}

pub trait InputMessageTrait<B, T> {
    fn update_buffer(&self, buffer: &mut B);

    fn create_message(buffer: &B, end_tick: Tick, num_ticks: u16) -> Self;
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

    /// We received a new input message from the user, and use it to update the input buffer
    /// TODO: should we keep track of which inputs in the input buffer are absent and only update those?
    ///  The current tick is the current server tick, no need to update the buffer for ticks that are older than that
    pub(crate) fn update_buffer(&self, buffer: &mut InputBuffer<T>) {
        let message_start_tick = Tick(self.end_tick.0) - self.inputs.len() as u16 + 1;

        for (delta, input) in self.inputs.iter().enumerate() {
            let tick = message_start_tick + Tick(delta as u16);
            match input {
                InputData::Absent => {
                    buffer.set_empty(tick);
                }
                InputData::SameAsPrecedent => {
                    if let Some(v) = buffer.get(tick-1) {
                        buffer.set(tick, v.clone());
                    } else {
                        buffer.set_empty(tick);
                    }
                }
                InputData::Input(input) => {
                    if buffer
                        .get(tick)
                        .is_some_and(|existing_value| existing_value == input)
                    {
                        continue;
                    }
                    buffer.set(tick, input.clone());
                }
            }
        }
    }

        /// Add the inputs for the `num_ticks` ticks starting from `self.end_tick - num_ticks + 1` up to `self.end_tick`
    ///
    /// If we don't have a starting `ActionState` from the `input_buffer`, we start from the first tick for which
    /// we have an `ActionState`.
    pub fn add_inputs(
            &mut self,
            num_ticks: u16,
            target: InputTarget,
            input_buffer: &InputBuffer<T>,
    ) {
        let Some(buffer_start_tick) = input_buffer.start_tick else {
            return
        };
        // find the first tick for which we have an `ActionState` buffered
        let mut start_tick = max(self.end_tick - num_ticks + 1, buffer_start_tick);

        // find the initial state
        let start_state = input_buffer.get(start_tick).map_or(
            InputData::Absent,
            |input| InputData::Input(input.clone()),
        );
        let mut states = vec![start_state];
        // append the other states until the end tick
        let buffer_start = (start_tick + 1 - buffer_start_tick) as usize;
        let buffer_end = (self.end_tick + 1 - buffer_start_tick) as usize;
        states.extend_from_slice(&input_buffer.buffer[buffer_start..buffer_end]);
        self.inputs.push(PerTargetData::<T> {
            target,
            states
        });
    }

    // Convert the last N ticks up to end_tick included into a compressed message that we can send to the server
    // Return None if the last N inputs are all Absent
    pub(crate) fn create_message(buffer: &InputBuffer<T>, end_tick: Tick, num_ticks: u16) -> InputMessage<T> {
        let mut inputs = Vec::new();
        // start with the first value
        let start_tick = Tick(end_tick.0) - num_ticks + 1;
        inputs.push(
            buffer.get(start_tick)
                .map_or(InputData::Absent, |input| InputData::Input(input.clone())),
        );
        // keep track of the previous value to avoid sending the same value multiple times
        let mut prev_value_idx = 0;
        for delta in 1..num_ticks {
            let tick = start_tick + Tick(delta);
            // safe because we keep pushing elements
            let value = buffer
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
        InputMessage { interpolation_delay: None, inputs, end_tick }
    }
}


#[cfg(test)]
mod tests {
    use crate::inputs::native::input_buffer::{InputBuffer, InputData};
    use crate::inputs::native::input_message::InputMessage;
    use crate::prelude::Tick;

    #[test]
    fn test_create_message() {
        let mut input_buffer = InputBuffer::default();

        input_buffer.set(Tick(4), 0);
        input_buffer.set(Tick(6), 1);
        input_buffer.set(Tick(7), 1);

        let message = InputMessage::create_message(&input_buffer, Tick(10), 8);
        assert_eq!(
            message,
            InputMessage {
                interpolation_delay: None,
                end_tick: Tick(10),
                inputs: vec![
                    InputData::Absent,
                    InputData::Input(0),
                    InputData::SameAsPrecedent,
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
            interpolation_delay: None,
            end_tick: Tick(20),
            inputs: vec![
                InputData::Absent,
                InputData::Input(0),
                InputData::SameAsPrecedent,
                InputData::Input(1),
                InputData::SameAsPrecedent,
                InputData::Absent,
                InputData::SameAsPrecedent,
                InputData::SameAsPrecedent,
            ],
        };
        message.update_buffer(&mut input_buffer);

        assert_eq!(input_buffer.get(Tick(20)), None);
        assert_eq!(input_buffer.get(Tick(19)), None);
        assert_eq!(input_buffer.get(Tick(18)), None);
        assert_eq!(input_buffer.get(Tick(17)), Some(&1));
        assert_eq!(input_buffer.get(Tick(16)), Some(&1));
        assert_eq!(input_buffer.get(Tick(15)), Some(&0));
        assert_eq!(input_buffer.get(Tick(14)), Some(&0));
        assert_eq!(input_buffer.get(Tick(13)), None);
    }
}


