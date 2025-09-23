use crate::action_state::{ActionState, InputMarker};
use alloc::{vec, vec::Vec};
use bevy_ecs::entity::{EntityMapper, MapEntities};
use bevy_reflect::{FromReflect, Reflect, Reflectable};
use core::cmp::max;
use core::fmt::Debug;
use core::time::Duration;
use lightyear_core::prelude::Tick;
use lightyear_inputs::input_buffer::{InputBuffer, InputData};
use lightyear_inputs::input_message::{ActionStateSequence, InputSnapshot};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

pub type SnapshotBuffer<A> = InputBuffer<ActionState<A>>;

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Reflect)]
pub struct NativeStateSequence<A> {
    states: Vec<InputData<A>>,
}

impl<A: Debug + PartialEq + Clone + Send + Sync + 'static> InputSnapshot for ActionState<A> {
    type Action = A;

    fn decay_tick(&mut self, tick_duration: Duration) {}
}

impl<A> IntoIterator for NativeStateSequence<A> {
    type Item = InputData<ActionState<A>>;
    type IntoIter =
        core::iter::Map<vec::IntoIter<InputData<A>>, fn(InputData<A>) -> InputData<ActionState<A>>>;

    fn into_iter(self) -> Self::IntoIter {
        self.states.into_iter().map(|input| match input {
            InputData::Absent => InputData::Absent,
            InputData::SameAsPrecedent => InputData::SameAsPrecedent,
            InputData::Input(i) => InputData::Input(ActionState(i)),
        })
    }
}

impl<
    A: Serialize
        + DeserializeOwned
        + Clone
        + PartialEq
        + Send
        + Sync
        + Debug
        + Default
        + Reflectable
        + FromReflect
        + 'static,
> ActionStateSequence for NativeStateSequence<A>
{
    type Action = A;
    type Snapshot = ActionState<A>;
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

    fn get_snapshots_from_message(self) -> impl Iterator<Item = InputData<Self::Snapshot>> {
        self.states.into_iter().map(|input| match input {
            InputData::Absent => InputData::Absent,
            InputData::SameAsPrecedent => InputData::SameAsPrecedent,
            InputData::Input(i) => InputData::Input(ActionState(i)),
        })
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

    fn to_snapshot(state: &ActionState<A>) -> Self::Snapshot {
        (*state).clone()
    }

    fn from_snapshot(state: &mut ActionState<A>, snapshot: &Self::Snapshot) {
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
    use alloc::collections::VecDeque;
    use std::time::Duration;

    #[test]
    fn test_build_sequence_from_buffer() {
        let mut input_buffer = InputBuffer::default();
        input_buffer.set_empty(Tick(2));
        input_buffer.set(Tick(3), ActionState(1));
        input_buffer.set(Tick(7), ActionState(2));

        let sequence =
            NativeStateSequence::<usize>::build_from_input_buffer(&input_buffer, 9, Tick(10))
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
        sequence.update_buffer(
            &mut input_buffer,
            Tick(20),
            TickDuration(Duration::default()),
        );
        assert_eq!(input_buffer.get(Tick(20)), None,);
        assert_eq!(input_buffer.get(Tick(19)), None,);
        assert_eq!(input_buffer.get(Tick(18)), None,);
        assert_eq!(input_buffer.get(Tick(17)), Some(&ActionState::<i32>(1)));
        assert_eq!(input_buffer.get(Tick(16)), Some(&ActionState::<i32>(1)));
        assert_eq!(input_buffer.get(Tick(15)), Some(&ActionState::<i32>(0)));
        assert_eq!(input_buffer.get(Tick(14)), Some(&ActionState::<i32>(0)));
        assert_eq!(input_buffer.get(Tick(13)), None,);
    }

    /// Test that the sequence updates the buffer correctly when the sequence's start tick is lower than the input
    /// buffer's start tick
    #[test]
    fn test_update_buffer_from_sequence_lower_start_tick() {
        let mut input_buffer = InputBuffer {
            start_tick: Some(Tick(10)),
            buffer: VecDeque::from([
                InputData::Input(ActionState(0)),
                InputData::SameAsPrecedent,
                InputData::SameAsPrecedent,
            ]),
        };
        let sequence = NativeStateSequence::<usize> {
            states: vec![
                // tick 7
                InputData::Absent,
                InputData::SameAsPrecedent,
                // tick 9
                InputData::Input(0),
                InputData::SameAsPrecedent,
                InputData::SameAsPrecedent,
                InputData::SameAsPrecedent,
                InputData::SameAsPrecedent,
                InputData::Input(1),
            ],
        };
        let mismatch = sequence.update_buffer(
            &mut input_buffer,
            Tick(14),
            TickDuration(Duration::default()),
        );
        assert_eq!(mismatch, Some(Tick(14)));
        assert_eq!(input_buffer.get(Tick(14)), Some(&ActionState(1)));
        assert_eq!(input_buffer.get(Tick(13)), Some(&ActionState(0)));
        assert_eq!(input_buffer.get(Tick(12)), Some(&ActionState(0)));
        assert_eq!(input_buffer.get(Tick(11)), Some(&ActionState(0)));
        assert_eq!(input_buffer.get(Tick(10)), Some(&ActionState(0)));
        assert_eq!(input_buffer.get(Tick(9)), None);
        assert_eq!(input_buffer.get(Tick(8)), None);
        assert_eq!(input_buffer.get(Tick(7)), None);
    }

    #[test]
    fn test_update_buffer_from_sequence_absent() {
        let mut input_buffer = InputBuffer {
            start_tick: Some(Tick(10)),
            buffer: VecDeque::from([
                InputData::Input(ActionState(0)),
                InputData::Absent,
                InputData::SameAsPrecedent,
            ]),
        };
        let sequence = NativeStateSequence::<usize> {
            states: vec![
                // Tick 11
                InputData::Absent,
                // Tick 12
                InputData::SameAsPrecedent,
            ],
        };
        sequence.update_buffer(
            &mut input_buffer,
            Tick(12),
            TickDuration(Duration::default()),
        );
        assert_eq!(input_buffer.get(Tick(12)), None);
        assert_eq!(input_buffer.get(Tick(11)), None);
        assert_eq!(input_buffer.get(Tick(10)), Some(&ActionState(0)));
    }

    #[test]
    fn test_update_buffer_from_sequence_present() {
        let mut input_buffer = InputBuffer {
            start_tick: Some(Tick(10)),
            buffer: VecDeque::from([InputData::Absent, InputData::SameAsPrecedent]),
        };
        let sequence = NativeStateSequence::<usize> {
            states: vec![
                // Tick 9
                InputData::Input(0),
                // Tick 10
                InputData::Absent,
                // Tick 11
                InputData::SameAsPrecedent,
            ],
        };
        let mismatch = sequence.update_buffer(
            &mut input_buffer,
            Tick(11),
            TickDuration(Duration::default()),
        );
        assert_eq!(mismatch, None);
        assert_eq!(input_buffer.get(Tick(11)), None);
        assert_eq!(input_buffer.get(Tick(10)), None);
        assert_eq!(input_buffer.get(Tick(9)), None);
    }
}
