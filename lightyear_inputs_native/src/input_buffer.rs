use crate::action_state::ActionState;
use lightyear_core::prelude::Tick;
use lightyear_inputs::input_buffer::{InputBuffer, InputData};
use lightyear_inputs::UserAction;

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