use crate::inputs::native::input_buffer::{InputBuffer, InputData};
use crate::prelude::{Deserialize, Serialize, Tick, UserAction};
use bevy::ecs::entity::MapEntities;
use bevy::prelude::{Entity, EntityMapper, Reflect};
use std::fmt::{Formatter, Write};

// TODO: use Mode to specify how to serialize a message (serde vs bitcode)! + can specify custom serialize function as well (similar to interpolation mode)
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Reflect)]
/// Message that we use to send the client inputs to the server
/// We will store the last N inputs starting from start_tick (in case of packet loss)
pub struct InputMessage<T> {
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
        InputMessage { inputs, end_tick }
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


