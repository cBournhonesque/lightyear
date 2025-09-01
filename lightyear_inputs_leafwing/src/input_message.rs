use bevy_platform::time::Instant;

use crate::action_diff::ActionDiff;
use crate::action_state::{ActionStateWrapper, ActionStateWrapperReadOnlyItem, LeafwingUserAction};
use alloc::vec::Vec;
use bevy_derive::{Deref, DerefMut};
use bevy_ecs::entity::{EntityMapper, MapEntities};
use core::time::Duration;
use leafwing_input_manager::Actionlike;
use leafwing_input_manager::action_state::ActionState;
use leafwing_input_manager::input_map::InputMap;
use lightyear_core::prelude::Tick;
use lightyear_inputs::input_buffer::{InputBuffer, InputData};
use lightyear_inputs::input_message::{ActionStateSequence, InputSnapshot};
use serde::{Deserialize, Serialize};

pub type SnapshotBuffer<A> = InputBuffer<LeafwingSnapshot<A>>;
impl<A: LeafwingUserAction> InputSnapshot for LeafwingSnapshot<A> {
    type Action = A;

    fn decay_tick(&mut self, tick_duration: Duration) {
        self.tick(Instant::now(), Instant::now() + tick_duration);
    }
}

#[derive(Debug, Clone, PartialEq, Deref, DerefMut)]
pub struct LeafwingSnapshot<A: LeafwingUserAction>(pub ActionState<A>);

impl<A: LeafwingUserAction> From<ActionState<A>> for LeafwingSnapshot<A> {
    fn from(state: ActionState<A>) -> Self {
        Self(state)
    }
}

impl<A: LeafwingUserAction> Default for LeafwingSnapshot<A> {
    fn default() -> Self {
        Self(ActionState::default())
    }
}

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
    type Snapshot = LeafwingSnapshot<A>;
    type State = ActionStateWrapper<A>;
    type Marker = InputMap<A>;

    fn is_empty(&self) -> bool {
        self.diffs
            .iter()
            .all(|diffs_per_tick| diffs_per_tick.is_empty())
    }

    fn len(&self) -> usize {
        self.diffs.len() + 1
    }

    fn get_snapshots_from_message(self) -> impl Iterator<Item = InputData<Self::Snapshot>> {
        let start_iter =
            core::iter::once(InputData::Input(LeafwingSnapshot(self.start_state.clone())));
        let diffs_iter = self.diffs.into_iter().scan(
            self.start_state,
            |state: &mut ActionState<A>, diffs_for_tick: Vec<ActionDiff<A>>| {
                // TODO: there's an issue; we use the diffs to set future ticks after the start value, but those values
                //  have not been ticked correctly! As a workaround, we tick them manually so that JustPressed becomes Pressed,
                //  but it will NOT work for timing-related features
                state.tick(Instant::now(), Instant::now());
                for diff in diffs_for_tick {
                    diff.apply(state);
                }
                state.set_update_state_from_state();
                state.set_fixed_update_state_from_state();

                // TODO: how can we check if the state is the same as before, to return InputData::SameAsPrecedent instead?
                Some(InputData::Input(LeafwingSnapshot(state.clone())))
            },
        );
        start_iter.chain(diffs_iter)
    }

    /// Add the inputs for the `num_ticks` ticks starting from `self.end_tick - num_ticks + 1` up to `self.end_tick`
    ///
    /// If we don't have a starting `ActionState` from the `input_buffer`, we start from the first tick for which
    /// we have an `ActionState`.
    fn build_from_input_buffer<'w, 's>(
        input_buffer: &InputBuffer<Self::Snapshot>,
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
                    .unwrap_or(&LeafwingSnapshot::<A>::default()),
                input_buffer
                    .get(tick)
                    .unwrap_or(&LeafwingSnapshot::<A>::default()),
            );
            diffs.push(diffs_for_tick);
            tick += 1;
        }
        Some(Self {
            start_state: start_state.0,
            diffs,
        })
    }

    fn to_snapshot<'w, 's>(state: ActionStateWrapperReadOnlyItem<A>) -> Self::Snapshot {
        LeafwingSnapshot(state.inner.clone())
    }

    fn from_snapshot<'w, 's>(state: &mut ActionState<A>, snapshot: &Self::Snapshot) {
        *state = snapshot.0.clone();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use alloc::vec;
    use bevy_reflect::Reflect;
    use leafwing_input_manager::Actionlike;
    use serde::{Deserialize, Serialize};
    use std::time::Duration;
    use test_log::test;

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
        input_buffer.set(Tick(2), ActionState::default().into());
        action_state.press(&Action::Jump);
        input_buffer.set(Tick(3), action_state.clone().into());
        action_state.release(&Action::Jump);
        input_buffer.set(Tick(7), action_state.clone().into());

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
        let input_buffer: InputBuffer<_> = InputBuffer::default();
        let sequence =
            LeafwingSequence::<Action>::build_from_input_buffer(&input_buffer, 5, Tick(10));
        assert!(sequence.is_none());
    }

    #[test]
    fn test_build_from_input_buffer_partial_overlap() {
        let mut input_buffer = InputBuffer::default();
        let mut action_state = ActionState::<Action>::default();
        action_state.press(&Action::Jump);
        input_buffer.set(Tick(8), action_state.clone().into());
        action_state.release(&Action::Jump);
        action_state.press(&Action::Run);
        input_buffer.set(Tick(10), action_state.clone().into());

        // Only ticks 8 and 10 are set, so sequence should start at 8
        let sequence =
            LeafwingSequence::<Action>::build_from_input_buffer(&input_buffer, 5, Tick(12))
                .unwrap();
        assert_eq!(
            sequence.start_state,
            input_buffer.get(Tick(8)).unwrap().clone().0
        );
        assert_eq!(sequence.diffs.len(), 4);
    }

    #[test]
    fn test_update_buffer_extends_left_and_right() {
        let mut input_buffer = InputBuffer::default();
        let mut action_state = ActionState::<Action>::default();
        action_state.press(&Action::Jump);
        input_buffer.set(Tick(6), action_state.clone().into());
        let sequence = LeafwingSequence::<Action> {
            // Tick 5
            start_state: action_state.clone(),
            diffs: vec![
                // Tick 6
                vec![],
                // Tick 7
                vec![ActionDiff::Pressed {
                    action: Action::Run,
                }],
                // Tick 8
                vec![ActionDiff::Released {
                    action: Action::Jump,
                }],
            ],
        };
        let mismatch = sequence.update_buffer(
            &mut input_buffer,
            Tick(8),
            TickDuration(Duration::default()),
        );
        assert_eq!(mismatch, Some(Tick(7)));

        // NOTE: The action_state from the sequence are ticked to avoid having JustPressed on each tick!
        let mut expected = action_state.clone();
        assert_eq!(input_buffer.get(Tick(6)).unwrap().0, expected);

        expected.tick(Instant::now(), Instant::now());
        expected.press(&Action::Run);
        expected.set_update_state_from_state();
        expected.set_fixed_update_state_from_state();
        assert_eq!(input_buffer.get(Tick(7)).unwrap().0, expected);

        expected.tick(Instant::now(), Instant::now());
        expected.release(&Action::Jump);
        expected.set_update_state_from_state();
        expected.set_fixed_update_state_from_state();
        assert_eq!(input_buffer.get(Tick(8)).unwrap().0, expected);
    }

    #[test]
    fn test_update_buffer_overwrites_existing() {
        let mut input_buffer = InputBuffer::default();
        let mut action_state = ActionState::<Action>::default();
        action_state.press(&Action::Jump);
        input_buffer.set(Tick(2), action_state.clone().into());
        let sequence = LeafwingSequence::<Action> {
            start_state: action_state.clone(),
            diffs: vec![vec![ActionDiff::Released {
                action: Action::Jump,
            }]],
        };
        // Should overwrite tick 3
        sequence.update_buffer(
            &mut input_buffer,
            Tick(3),
            TickDuration(Duration::default()),
        );
        assert_eq!(input_buffer.get(Tick(2)).unwrap().0, action_state);

        let mut expected = action_state.clone();
        expected.release(&Action::Jump);
        expected.set_update_state_from_state();
        expected.set_fixed_update_state_from_state();
        assert_eq!(input_buffer.get(Tick(3)).unwrap().0, expected);
    }
}
