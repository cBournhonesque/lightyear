use crate::action_state::{ActionState, InputMarker};
#[cfg(not(feature = "std"))]
use alloc::{vec, vec::Vec};
use bevy::ecs::entity::MapEntities;
use bevy::ecs::system::SystemParam;
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

impl<
    A: Serialize
        + DeserializeOwned
        + Clone
        + PartialEq
        + Send
        + Sync
        + Debug
        + Reflectable
        + FromReflect
        + 'static,
> ActionStateSequence for NativeStateSequence<A>
{
    type Action = A;
    type Snapshot = ActionState<A>;
    type State = ActionState<A>;
    type Marker = InputMarker<A>;

    type Context = ();

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

    fn update_buffer<'w, 's>(self, input_buffer: &mut InputBuffer<Self::State>, end_tick: Tick) {
        let start_tick = end_tick + 1 - self.len() as u16;
        // the first value is guaranteed to not be SameAsPrecedent
        for (delta, input) in self.states.into_iter().enumerate() {
            let tick = start_tick + Tick(delta as u16);
            match input {
                InputData::Absent => {
                    input_buffer.set_raw(tick, InputData::Absent);
                }
                InputData::SameAsPrecedent => {
                    input_buffer.set_raw(tick, InputData::SameAsPrecedent);
                }
                InputData::Input(input) => {
                    // do not set the value if it's equal to what's already in the buffer
                    if input_buffer.get(tick).is_some_and(|existing_value| {
                        existing_value.value.as_ref().is_some_and(|v| v == &input)
                    }) {
                        continue;
                    }
                    input_buffer.set(tick, ActionState::<A> { value: Some(input) });
                }
            }
        }
    }

    fn build_from_input_buffer<'w, 's>(
        input_buffer: &InputBuffer<Self::State>,
        num_ticks: u16,
        end_tick: Tick,
    ) -> Option<Self> {
        let buffer_start_tick = input_buffer.start_tick?;
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

    fn to_snapshot<'w, 's>(
        state: &Self::State,
        _: &<Self::Context as SystemParam>::Item<'w, 's>,
    ) -> Self::Snapshot {
        state.clone()
    }

    fn from_snapshot<'w, 's>(
        state: &mut Self::State,
        snapshot: &Self::Snapshot,
        _: &<Self::Context as SystemParam>::Item<'w, 's>,
    ) {
        *state = snapshot.clone();
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

    #[test]
    fn test_build_sequence_from_buffer() {
        let mut input_buffer = InputBuffer::default();
        input_buffer.set(Tick(2), ActionState { value: None });
        input_buffer.set(Tick(3), ActionState { value: Some(1) });
        input_buffer.set(Tick(7), ActionState { value: Some(2) });

        let sequence =
            NativeStateSequence::<usize>::build_from_input_buffer(&input_buffer, 9, Tick(10), &())
                .unwrap();
        assert_eq!(
            sequence,
            NativeStateSequence::<usize> {
                states: vec![
                    // tick 2
                    InputData::Absent,
                    // tick 3
                    InputData::Input(1),
                    InputData::SameAsPrecedent,
                    InputData::SameAsPrecedent,
                    InputData::SameAsPrecedent,
                    InputData::Input(2),
                    // TODO: why is it marked as absent instead of SameAsPrecedent??
                    //  by default, when inputs are absent should we mark them as SameAsPrecedent?
                    InputData::Absent,
                    InputData::Absent,
                    InputData::Absent,
                ],
            }
        );
    }

    #[test]
    fn test_update_buffer_from_sequence() {
        let mut input_buffer = InputBuffer::default();
        let sequence = NativeStateSequence::<i32> {
            states: vec![
                // tick 13
                InputData::Absent,
                // tick 14
                InputData::Input(0),
                InputData::SameAsPrecedent,
                // tick 16
                InputData::Input(1),
                InputData::SameAsPrecedent,
                // tick 18
                InputData::Absent,
                InputData::SameAsPrecedent,
                // Tick 20
                InputData::SameAsPrecedent,
            ],
        };
        sequence.update_buffer(&mut input_buffer, Tick(20), &());
        assert_eq!(input_buffer.get(Tick(20)), None,);
        assert_eq!(input_buffer.get(Tick(19)), None,);
        assert_eq!(input_buffer.get(Tick(18)), None,);
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
        assert_eq!(input_buffer.get(Tick(13)), None,);
    }
}
