use crate::inputs::native::input_buffer::{InputBuffer, InputData};
use crate::inputs::native::ActionState;
use crate::prelude::client::InterpolationDelay;
use crate::prelude::{Deserialize, Serialize, Tick, UserAction};
#[cfg(not(feature = "std"))]
use alloc::{format, string::String, vec, vec::Vec};
use bevy::ecs::entity::MapEntities;
use bevy::prelude::{Entity, EntityMapper, Reflect};
use core::cmp::max;
use core::fmt::{Formatter, Write};

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
pub(crate) struct PerTargetData<A> {
    pub(crate) target: InputTarget,
    // ActionState<A> from ticks `end_ticks-N` to `end_tick` (included)
    pub(crate) states: Vec<InputData<A>>,
}
impl<A: UserAction + MapEntities> MapEntities for InputMessage<A> {
    // NOTE: we do NOT map the entities for input-message because when already convert
    //  the entities on the message to the corresponding client entities when we write them
    //  in the input message

    // NOTE: we only map the inputs for the pre-predicted entities
    fn map_entities<M: EntityMapper>(&mut self, entity_mapper: &mut M) {
        self.inputs.iter_mut().for_each(|data| {
            if let InputTarget::PrePredictedEntity(e) = &mut data.target {
                *e = entity_mapper.get_mapped(*e);
            }
            data.states.iter_mut().for_each(|state| {
                if let InputData::Input(action_state) = state {
                    action_state.map_entities(entity_mapper);
                }
            });
        });
    }
}

impl<A: UserAction> core::fmt::Display for InputMessage<A> {
    fn fmt(&self, f: &mut Formatter<'_>) -> core::fmt::Result {
        let ty = core::any::type_name::<A>();

        if self.inputs.is_empty() {
            return write!(f, "EmptyInputMessage");
        }
        let start_tick = self.end_tick - Tick(self.inputs[0].states.len() as u16);
        let buffer_str = self
            .inputs
            .iter()
            .map(|data| {
                let mut str = format!("Entity: {:?}\n", data.target);
                for (i, input) in data.states.iter().enumerate() {
                    let tick = start_tick + i as i16;
                    let _ = writeln!(&mut str, "Tick: {}, Input: {:?}", tick, input);
                }
                str
            })
            .collect::<Vec<String>>()
            .join("\n");
        write!(f, "InputMessage<{:?}>:\n {}", ty, buffer_str)
    }
}

impl<T: UserAction> InputMessage<T> {
    pub fn new(end_tick: Tick) -> Self {
        Self {
            interpolation_delay: None,
            end_tick,
            inputs: vec![],
        }
    }

    pub fn is_empty(&self) -> bool {
        self.inputs.iter().all(|data| {
            data.states.is_empty()
                || data
                    .states
                    .iter()
                    .all(|s| matches!(s, InputData::Absent | InputData::SameAsPrecedent))
        })
    }

    /// Add the inputs for the `num_ticks` ticks starting from `self.end_tick - num_ticks + 1` up to `self.end_tick`
    ///
    /// If we don't have a starting `ActionState` from the `input_buffer`, we start from the first tick for which
    /// we have an `ActionState`.
    pub fn add_inputs(
        &mut self,
        num_ticks: u16,
        target: InputTarget,
        input_buffer: &InputBuffer<ActionState<T>>,
    ) {
        let Some(buffer_start_tick) = input_buffer.start_tick else {
            return;
        };
        // find the first tick for which we have an `ActionState` buffered
        let start_tick = max(self.end_tick - num_ticks + 1, buffer_start_tick);

        // find the initial state, (which we convert out of SameAsPrecedent)
        let start_state = input_buffer
            .get(start_tick)
            .map_or(InputData::Absent, |input| input.into());
        let mut states = vec![start_state];

        // append the other states until the end tick
        let buffer_start = (start_tick + 1 - buffer_start_tick) as usize;
        let buffer_end = (self.end_tick + 1 - buffer_start_tick) as usize;
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
        self.inputs.push(PerTargetData::<T> { target, states });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_message() {
        let mut input_buffer = InputBuffer::default();

        input_buffer.set(Tick(4), ActionState { value: Some(0) });
        input_buffer.set(Tick(6), ActionState { value: Some(1) });
        input_buffer.set(Tick(7), ActionState { value: Some(1) });

        let mut message = InputMessage::<u8> {
            interpolation_delay: None,
            end_tick: Tick(10),
            inputs: vec![],
        };
        message.add_inputs(8, InputTarget::Entity(Entity::PLACEHOLDER), &input_buffer);
        assert_eq!(
            message,
            InputMessage {
                interpolation_delay: None,
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
