use crate::action_state::ActionState;
#[cfg(not(feature = "std"))]
use alloc::vec::Vec;
use lightyear_core::prelude::Tick;
use lightyear_inputs::input_buffer::{InputBuffer, InputData};
use lightyear_inputs::UserAction;


/// Upon receiving an [`InputMessage`](super::input_message::InputMessage), update the InputBuffer with all the inputs
/// included in the message.
pub(crate) fn update_from_message<T: UserAction>(
    input_buffer: &mut InputBuffer<ActionState<T>>,
    end_tick: Tick,
    values: &Vec<InputData<T>>,
) {
    let start_tick = end_tick + 1 - values.len() as u16;
        // the first value is guaranteed to not be SameAsPrecedent
        for (delta, input) in values.iter().enumerate() {
            let tick = start_tick + Tick(delta as u16);
            match input {
                InputData::Absent => {
                    input_buffer.set_raw(tick, InputData::Input(ActionState::<T> { value: None }));
                }
                InputData::SameAsPrecedent => {
                    input_buffer.set_raw(tick, InputData::SameAsPrecedent);
                }
                InputData::Input(input) => {
                    // do not set the value if it's equal to what's already in the buffer
                    if input_buffer.get(tick).is_some_and(|existing_value| {
                        existing_value.value.as_ref().is_some_and(|v| v == input)
                    }) {
                        continue;
                    }
                    input_buffer.set(
                        tick,
                        ActionState::<T> {
                            value: Some(input.clone()),
                        },
                    );
                }
            }
        }
}