use crate::action_state::{ActionState, InputMarker};
#[cfg(not(feature = "std"))]
use alloc::{vec, vec::Vec};
use bevy::ecs::entity::MapEntities;
use bevy::prelude::{EntityMapper, FromReflect, Reflect};
use bevy::reflect::Reflectable;
use core::cmp::max;
use core::fmt::Debug;
use lightyear_core::prelude::Tick;
use lightyear_inputs::input_buffer::{InputBuffer, InputData};
use lightyear_inputs::input_message::ActionStateSequence;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Reflect)]
pub struct NativeStateSequence<A> {
    states: Vec<InputData<A>>,
}

impl<A: Serialize + DeserializeOwned + Clone + PartialEq + Send + Sync + Debug + Reflectable + FromReflect + 'static>
    ActionStateSequence for NativeStateSequence<A>
{
    type Action = A;
    type State = ActionState<A>;

    type Marker = InputMarker<A>;

    fn is_empty(&self) -> bool {
        self.states.is_empty()
            || self
                .states
                .iter()
                .all(|s| matches!(s, InputData::Absent | InputData::SameAsPrecedent))
    }

    fn len(&self) -> usize {
        self.states.len()
    }

    fn update_buffer(&self, input_buffer: &mut InputBuffer<Self::State>, end_tick: Tick) {
        let start_tick = end_tick + 1 - self.len() as u16;
        // the first value is guaranteed to not be SameAsPrecedent
        for (delta, input) in self.states.iter().enumerate() {
            let tick = start_tick + Tick(delta as u16);
            match input {
                InputData::Absent => {
                    input_buffer.set_raw(tick, InputData::Input(Self::State::default()));
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
                        ActionState::<A> {
                            value: Some(input.clone()),
                        },
                    );
                }
            }
        }
    }

    fn build_from_input_buffer(
        input_buffer: &InputBuffer<Self::State>,
        num_ticks: u16,
        end_tick: Tick,
    ) -> Option<Self> {
        let Some(buffer_start_tick) = input_buffer.start_tick else {
            return None;
        };
        // find the first tick for which we have an `ActionState` buffered
        let start_tick = max(end_tick - num_ticks + 1, buffer_start_tick);

        // find the initial state, (which we convert out of SameAsPrecedent)
        let start_state = input_buffer
            .get(start_tick)
            .map_or(InputData::Absent, |input| input.into());
        let mut states = vec![start_state];

        // append the other states until the end tick
        let buffer_start = (start_tick + 1 - buffer_start_tick) as usize;
        let buffer_end = (end_tick + 1 - buffer_start_tick) as usize;
        for idx in buffer_start..buffer_end {
            let state =
                input_buffer
                    .buffer
                    .get(idx)
                    .map_or(InputData::Absent, |input| match input {
                        InputData::Absent => InputData::Absent,
                        InputData::SameAsPrecedent => InputData::SameAsPrecedent,
                        InputData::Input(v) => v.into(),
                    });
            states.push(state);
        }
        Some(Self { states })
    }
}

impl<A: MapEntities> MapEntities for NativeStateSequence<A> {
    fn map_entities<E: EntityMapper>(&mut self, entity_mapper: &mut E) {
        self.states.iter_mut().for_each(|state| {
            if let InputData::Input(action_state) = state {
                action_state.map_entities(entity_mapper);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lightyear_inputs::input_message::{InputMessage, InputTarget, PerTargetData};

    #[test]
    fn test_create_message() {
        let mut input_buffer = InputBuffer::default();

        input_buffer.set(Tick(4), ActionState { value: Some(0) });
        input_buffer.set(Tick(6), ActionState { value: Some(1) });
        input_buffer.set(Tick(7), ActionState { value: Some(1) });

        let mut message = InputMessage::<u8> {
            // interpolation_delay: None,
            end_tick: Tick(10),
            inputs: vec![],
        };
        message.add_inputs(8, InputTarget::Entity(Entity::PLACEHOLDER), &input_buffer);
        assert_eq!(
            message,
            InputMessage {
                // interpolation_delay: None,
                end_tick: Tick(10),
                inputs: vec![PerTargetData {
                    target: InputTarget::Entity(Entity::PLACEHOLDER),
                    states: vec![
                        InputData::Input(0),
                        InputData::SameAsPrecedent,
                        InputData::Input(1),
                        InputData::SameAsPrecedent,
                        InputData::Absent,
                        InputData::Absent,
                        InputData::Absent,
                    ]
                },],
            }
        );
    }

    #[test]
    fn test_update_from_message() {
        let mut input_buffer = InputBuffer::default();
        input_buffer.update_from_message(
            Tick(20),
            &vec![
                InputData::Absent,
                InputData::Input(0),
                InputData::SameAsPrecedent,
                InputData::Input(1),
                InputData::SameAsPrecedent,
                InputData::Absent,
                InputData::SameAsPrecedent,
                InputData::SameAsPrecedent,
            ],
        );
        assert_eq!(
            input_buffer.get(Tick(20)),
            Some(&ActionState::<i32> { value: None })
        );
        assert_eq!(
            input_buffer.get(Tick(19)),
            Some(&ActionState::<i32> { value: None })
        );
        assert_eq!(
            input_buffer.get(Tick(18)),
            Some(&ActionState::<i32> { value: None })
        );
        assert_eq!(
            input_buffer.get(Tick(17)),
            Some(&ActionState::<i32> { value: Some(1) })
        );
        assert_eq!(
            input_buffer.get(Tick(16)),
            Some(&ActionState::<i32> { value: Some(1) })
        );
        assert_eq!(
            input_buffer.get(Tick(15)),
            Some(&ActionState::<i32> { value: Some(0) })
        );
        assert_eq!(
            input_buffer.get(Tick(14)),
            Some(&ActionState::<i32> { value: Some(0) })
        );
        assert_eq!(
            input_buffer.get(Tick(13)),
            Some(&ActionState::<i32> { value: None })
        );
    }
}
