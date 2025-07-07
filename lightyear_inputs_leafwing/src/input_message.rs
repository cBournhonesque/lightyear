use bevy_platform::time::Instant;

use crate::action_diff::ActionDiff;
use crate::action_state::LeafwingUserAction;
use alloc::vec::Vec;
use bevy_ecs::{
    entity::{EntityMapper, MapEntities},
    system::SystemParam,
};
use leafwing_input_manager::Actionlike;
use leafwing_input_manager::action_state::ActionState;
use leafwing_input_manager::input_map::InputMap;
use lightyear_core::prelude::Tick;
use lightyear_inputs::input_buffer::InputBuffer;
use lightyear_inputs::input_message::ActionStateSequence;
use serde::{Deserialize, Serialize};
use tracing::trace;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LeafwingSequence<A: Actionlike> {
    pub(crate) start_state: ActionState<A>,
    pub(crate) diffs: Vec<Vec<ActionDiff<A>>>,
}

impl<A: LeafwingUserAction> MapEntities for LeafwingSequence<A> {
    fn map_entities<M: EntityMapper>(&mut self, _: &mut M) {}
}

impl<A: LeafwingUserAction> ActionStateSequence for LeafwingSequence<A> {
    type Action = A;
    type Snapshot = ActionState<A>;
    type State = ActionState<A>;
    type Marker = InputMap<A>;
    type Context = ();

    fn is_empty(&self) -> bool {
        self.diffs
            .iter()
            .all(|diffs_per_tick| diffs_per_tick.is_empty())
    }

    fn len(&self) -> usize {
        self.diffs.len()
    }

    fn update_buffer<'w, 's>(self, input_buffer: &mut InputBuffer<Self::State>, end_tick: Tick) {
        let start_tick = end_tick - self.len() as u16;
        input_buffer.extend_to_range(start_tick, end_tick);

        input_buffer.set(start_tick, self.start_state.clone());

        let mut value = self.start_state.clone();
        for (delta, diffs_for_tick) in self.diffs.into_iter().enumerate() {
            // TODO: there's an issue; we use the diffs to set future ticks after the start value, but those values
            //  have not been ticked correctly! As a workaround, we tick them manually so that JustPressed becomes Pressed,
            //  but it will NOT work for timing-related features
            value.tick(Instant::now(), Instant::now());
            let tick = start_tick + Tick(1 + delta as u16);
            for diff in diffs_for_tick {
                // TODO: also handle timings!
                diff.apply(&mut value);
            }
            // make sure that we update the fixed update state!
            value.set_update_state_from_state();
            value.set_fixed_update_state_from_state();
            input_buffer.set(tick, value.clone());
            trace!(
                "updated from input-message tick: {:?}, value: {:?}",
                tick, value
            );
        }
    }

    /// Add the inputs for the `num_ticks` ticks starting from `self.end_tick - num_ticks + 1` up to `self.end_tick`
    ///
    /// If we don't have a starting `ActionState` from the `input_buffer`, we start from the first tick for which
    /// we have an `ActionState`.
    fn build_from_input_buffer<'w, 's>(
        input_buffer: &InputBuffer<Self::State>,
        num_ticks: u16,
        end_tick: Tick,
    ) -> Option<Self> {
        let mut diffs = Vec::new();
        // find the first tick for which we have an `ActionState` buffered
        let mut start_tick = end_tick - num_ticks + 1;
        while start_tick <= end_tick {
            if input_buffer.get(start_tick).is_some() {
                break;
            }
            start_tick += 1;
        }

        // there are no ticks for which we have an `ActionState` buffered, so we send nothing
        if start_tick > end_tick {
            return None;
        }
        let start_state = input_buffer.get(start_tick).unwrap().clone();
        let mut tick = start_tick + 1;
        while tick <= end_tick {
            let diffs_for_tick = ActionDiff::<A>::create(
                // TODO: if the input_delay changes, this could leave gaps in the InputBuffer, which we will fill with Default
                input_buffer
                    .get(tick - 1)
                    .unwrap_or(&ActionState::<A>::default()),
                input_buffer
                    .get(tick)
                    .unwrap_or(&ActionState::<A>::default()),
            );
            diffs.push(diffs_for_tick);
            tick += 1;
        }
        Some(Self { start_state, diffs })
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

#[cfg(test)]
mod tests {
    use super::*;

    use alloc::vec;
    use bevy_reflect::Reflect;
    use leafwing_input_manager::Actionlike;
    use serde::{Deserialize, Serialize};

    #[derive(
        Serialize, Deserialize, Copy, Clone, Eq, PartialEq, Debug, Hash, Reflect, Actionlike,
    )]
    enum Action {
        Jump,
        Run,
    }

    #[test]
    fn test_create_message() {
        let mut input_buffer = InputBuffer::default();
        let mut action_state = ActionState::<Action>::default();
        input_buffer.set(Tick(2), ActionState::default());
        action_state.press(&Action::Jump);
        input_buffer.set(Tick(3), action_state.clone());
        action_state.release(&Action::Jump);
        input_buffer.set(Tick(7), action_state.clone());

        let sequence =
            LeafwingSequence::<Action>::build_from_input_buffer(&input_buffer, 9, Tick(10))
                .unwrap();
        assert_eq!(
            sequence,
            LeafwingSequence::<Action> {
                // tick 2
                start_state: ActionState::default(),
                diffs: vec![
                    // tick 3
                    vec![ActionDiff::Pressed {
                        action: Action::Jump,
                    }],
                    vec![],
                    vec![],
                    vec![],
                    // tick 7
                    vec![ActionDiff::Released {
                        action: Action::Jump,
                    }],
                    vec![],
                    vec![],
                    vec![],
                ]
            }
        );
    }

    #[test]
    fn test_build_from_input_buffer_empty() {
        let input_buffer: InputBuffer<ActionState<Action>> = InputBuffer::default();
        let sequence =
            LeafwingSequence::<Action>::build_from_input_buffer(&input_buffer, 5, Tick(10));
        assert!(sequence.is_none());
    }

    #[test]
    fn test_build_from_input_buffer_partial_overlap() {
        let mut input_buffer = InputBuffer::default();
        let mut action_state = ActionState::<Action>::default();
        action_state.press(&Action::Jump);
        input_buffer.set(Tick(8), action_state.clone());
        action_state.release(&Action::Jump);
        action_state.press(&Action::Run);
        input_buffer.set(Tick(10), action_state.clone());

        // Only ticks 8 and 10 are set, so sequence should start at 8
        let sequence =
            LeafwingSequence::<Action>::build_from_input_buffer(&input_buffer, 5, Tick(12))
                .unwrap();
        assert_eq!(
            sequence.start_state,
            input_buffer.get(Tick(8)).unwrap().clone()
        );
        assert_eq!(sequence.diffs.len(), 4);
    }

    #[test]
    fn test_update_buffer_extends_left_and_right() {
        let mut input_buffer = InputBuffer::default();
        let mut action_state = ActionState::<Action>::default();
        action_state.press(&Action::Jump);
        input_buffer.set(Tick(6), action_state.clone());
        let sequence = LeafwingSequence::<Action> {
            // Tick 5
            start_state: action_state.clone(),
            diffs: vec![
                // Tick 6
                vec![ActionDiff::Pressed {
                    action: Action::Run,
                }],
                vec![],
                // Tick 8
                vec![ActionDiff::Released {
                    action: Action::Jump,
                }],
            ],
        };
        // This should extend the buffer to fit ticks 5..=8
        sequence.update_buffer(&mut input_buffer, Tick(8));
        assert_eq!(input_buffer.get(Tick(5)).unwrap(), &action_state);

        // NOTE: The action_state from the sequence are ticked to avoid having JustPressed on each tick!
        let mut expected = action_state.clone();
        expected.tick(Instant::now(), Instant::now());
        expected.press(&Action::Run);
        assert_eq!(input_buffer.get(Tick(6)).unwrap(), &expected);
        expected.tick(Instant::now(), Instant::now());
        assert_eq!(input_buffer.get(Tick(7)).unwrap(), &expected);
        expected.tick(Instant::now(), Instant::now());
        expected.release(&Action::Jump);
        assert_eq!(input_buffer.get(Tick(8)).unwrap(), &expected);
    }

    #[test]
    fn test_update_buffer_overwrites_existing() {
        let mut input_buffer = InputBuffer::default();
        let mut action_state = ActionState::<Action>::default();
        action_state.press(&Action::Jump);
        input_buffer.set(Tick(2), action_state.clone());
        let sequence = LeafwingSequence::<Action> {
            start_state: action_state.clone(),
            diffs: vec![vec![ActionDiff::Released {
                action: Action::Jump,
            }]],
        };
        // Should overwrite tick 3
        sequence.update_buffer(&mut input_buffer, Tick(3));
        assert_eq!(input_buffer.get(Tick(2)).unwrap(), &action_state);
        let mut expected = action_state.clone();
        expected.release(&Action::Jump);
        assert_eq!(input_buffer.get(Tick(3)).unwrap(), &expected);
    }
}
