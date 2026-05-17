use bevy_platform::time::Instant;

use crate::action_diff::ActionDiff;
use crate::action_state::{ActionStateWrapper, ActionStateWrapperReadOnlyItem, LeafwingUserAction};
use alloc::vec::Vec;
use bevy_derive::{Deref, DerefMut};
use bevy_ecs::entity::{EntityMapper, MapEntities};
use core::time::Duration;
use leafwing_input_manager::Actionlike;
use leafwing_input_manager::InputControlKind;
use leafwing_input_manager::action_state::{ActionKindData, ActionState};
use leafwing_input_manager::input_map::InputMap;
use lightyear_core::prelude::Tick;
use lightyear_inputs::input_buffer::{Compressed, InputBuffer};
use lightyear_inputs::input_message::{ActionStateSequence, InputSnapshot};
use serde::{Deserialize, Serialize};

pub type LeafwingBuffer<A> = InputBuffer<LeafwingSnapshot<A>, A>;
impl<A: LeafwingUserAction> InputSnapshot for LeafwingSnapshot<A> {
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

    fn len(&self) -> usize {
        self.diffs.len() + 1
    }

    fn get_snapshots_from_message(
        self,
        tick_duration: Duration,
    ) -> impl Iterator<Item = Compressed<Self::Snapshot>> {
        let start_iter = core::iter::once(Compressed::Input(LeafwingSnapshot(
            self.start_state.clone(),
        )));
        let diffs_iter = self.diffs.into_iter().scan(
            self.start_state,
            move |state: &mut ActionState<A>, diffs_for_tick: Vec<ActionDiff<A>>| {
                state.tick(Instant::now() + tick_duration, Instant::now());
                for diff in diffs_for_tick {
                    diff.apply(state);
                }
                state.set_update_state_from_state();
                state.set_fixed_update_state_from_state();

                // TODO: how can we check if the state is the same as before, to return InputData::SameAsPrecedent instead?
                Some(Compressed::Input(LeafwingSnapshot(state.clone())))
            },
        );
        start_iter.chain(diffs_iter)
    }

    /// Add the inputs for the `num_ticks` ticks starting from `self.end_tick - num_ticks + 1` up to `self.end_tick`
    ///
    /// If we don't have a starting `ActionState` from the `input_buffer`, we start from the first tick for which
    /// we have an `ActionState`.
    fn build_from_input_buffer<'w, 's>(
        input_buffer: &InputBuffer<Self::Snapshot, Self::Action>,
        num_ticks: u32,
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

    /// Apply snapshot with transition-aware button state handling.
    ///
    /// The wire format (`ActionDiff`) collapses `JustPressed` into `Pressed`, so
    /// raw-cloning a snapshot on the server loses `JustPressed`/`JustReleased`.
    ///
    /// This method instead:
    /// 1. Ticks the state (`JustPressed→Pressed`, `JustReleased→Released`)
    /// 2. For each action, compares current vs snapshot and calls `press()`/`release()`
    ///    to produce correct `JustPressed`/`JustReleased` transitions
    /// 3. Applies axis values directly
    fn from_snapshot_transitions<'w>(state: &mut ActionState<A>, snapshot: &Self::Snapshot) {
        // Advance button state machine so JustPressed→Pressed between consecutive
        // fixed ticks within a single frame (tick_action_state only runs in PreUpdate).
        state.tick(Instant::now(), Instant::now());

        let new = &snapshot.0;
        for (action, new_data) in new.all_action_data() {
            match &new_data.kind_data {
                ActionKindData::Button(new_button) => {
                    let is_pressed = new_button.pressed();
                    if is_pressed {
                        // press() sets JustPressed unless already Pressed
                        state.press(action);
                    } else {
                        // release() sets JustReleased unless already Released
                        state.release(action);
                    }
                }
                ActionKindData::Axis(axis_data) => {
                    state.set_value(action, axis_data.value);
                }
                ActionKindData::DualAxis(dual_data) => {
                    state.set_axis_pair(action, dual_data.pair);
                }
                ActionKindData::TripleAxis(triple_data) => {
                    state.set_axis_triple(action, triple_data.triple);
                }
            }
        }

        // Release any buttons present in current state but absent from snapshot
        let snapshot_keys: Vec<A> = new.keys();
        for action in state.keys() {
            if action.input_control_kind() != InputControlKind::Button {
                continue;
            }
            if !snapshot_keys.contains(&action) && state.pressed(&action) {
                state.release(&action);
            }
        }
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
        let input_buffer: InputBuffer<_, _> = InputBuffer::default();
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
        input_buffer.last_remote_tick = Some(Tick(6));
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
        let mismatch = sequence.update_buffer(&mut input_buffer, Tick(8), Duration::default());
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
        sequence.update_buffer(&mut input_buffer, Tick(3), Duration::default());
        assert_eq!(input_buffer.get(Tick(2)).unwrap().0, action_state);

        let mut expected = action_state.clone();
        expected.release(&Action::Jump);
        expected.set_update_state_from_state();
        expected.set_fixed_update_state_from_state();
        assert_eq!(input_buffer.get(Tick(3)).unwrap().0, expected);
    }

    /// Simulates the late-join rebroadcast scenario: client 1 holds [Jump]
    /// for several ticks, sends an InputMessage. A new client (empty buffer)
    /// receives the rebroadcast. The new client's buffer should contain
    /// the pressed action for all ticks in the message.
    #[test]
    fn test_rebroadcast_to_empty_buffer() {
        // Client 1 has been pressing Jump for ticks 5..10
        let mut sender_buffer = InputBuffer::default();
        let mut action_state = ActionState::<Action>::default();
        action_state.press(&Action::Jump);
        for tick in 5..=10 {
            sender_buffer.set(Tick(tick), action_state.clone().into());
        }

        // Build the message that client 1 sends (history_depth=6, end_tick=10)
        let sequence =
            LeafwingSequence::<Action>::build_from_input_buffer(&sender_buffer, 6, Tick(10))
                .unwrap();

        // New client receives it into an empty buffer
        let mut receiver_buffer: InputBuffer<LeafwingSnapshot<Action>, Action> =
            InputBuffer::default();
        let mismatch = sequence.update_buffer(&mut receiver_buffer, Tick(10), Duration::default());

        // Every tick should have Jump pressed
        for tick in 5..=10 {
            let snapshot = receiver_buffer
                .get(Tick(tick))
                .unwrap_or_else(|| panic!("missing input at tick {tick}"));
            assert!(
                snapshot.pressed(&Action::Jump),
                "Jump should be pressed at tick {tick}, got: {:?}",
                snapshot.get_pressed()
            );
        }

        // get_last should also return Jump pressed
        let last = receiver_buffer
            .get_last()
            .expect("buffer should not be empty");
        assert!(
            last.pressed(&Action::Jump),
            "get_last should return Jump pressed, got: {:?}",
            last.get_pressed()
        );
    }

    /// Like test_rebroadcast_to_empty_buffer, but the sender's buffer uses
    /// SameAsPrecedent compression (which happens when set() is called with
    /// the same value on consecutive ticks — the real-world case).
    #[test]
    fn test_rebroadcast_to_empty_buffer_with_compression() {
        let mut sender_buffer = InputBuffer::default();
        let mut action_state = ActionState::<Action>::default();
        action_state.press(&Action::Jump);
        // set() compresses consecutive identical values to SameAsPrecedent
        for tick in 5..=10 {
            sender_buffer.set(Tick(tick), LeafwingSnapshot(action_state.clone()));
        }
        // Verify compression is in effect
        assert!(
            sender_buffer.get(Tick(5)).is_some(),
            "tick 5 should be present"
        );
        assert!(
            sender_buffer.get(Tick(10)).is_some(),
            "tick 10 should be present (via SameAsPrecedent)"
        );

        let sequence =
            LeafwingSequence::<Action>::build_from_input_buffer(&sender_buffer, 6, Tick(10))
                .unwrap();

        let mut receiver_buffer: InputBuffer<LeafwingSnapshot<Action>, Action> =
            InputBuffer::default();
        sequence.update_buffer(&mut receiver_buffer, Tick(10), Duration::default());

        for tick in 5..=10 {
            let snapshot = receiver_buffer
                .get(Tick(tick))
                .unwrap_or_else(|| panic!("missing input at tick {tick}"));
            assert!(
                snapshot.pressed(&Action::Jump),
                "Jump should be pressed at tick {tick}, got: {:?}",
                snapshot.get_pressed()
            );
        }
    }

    /// Test that get_action_state (which uses get(), not get_predict()) returns
    /// the correct input for ticks within the buffer range after a rebroadcast.
    /// This simulates the late-join scenario where the client's current tick is
    /// within the buffer range.
    #[test]
    fn test_rebroadcast_get_within_range() {
        let mut sender_buffer = InputBuffer::default();
        let mut action_state = ActionState::<Action>::default();
        action_state.press(&Action::Jump);
        for tick in 80..=100 {
            sender_buffer.set(Tick(tick), LeafwingSnapshot(action_state.clone()));
        }

        let sequence =
            LeafwingSequence::<Action>::build_from_input_buffer(&sender_buffer, 20, Tick(100))
                .unwrap();

        let mut receiver_buffer: InputBuffer<LeafwingSnapshot<Action>, Action> =
            InputBuffer::default();
        sequence.update_buffer(&mut receiver_buffer, Tick(100), Duration::default());

        // Message covers ticks 81..=100 (20 ticks), not 80
        // Simulate what the client does: get(tick) for current simulation tick
        for tick in 81..=100 {
            let snapshot = receiver_buffer.get(Tick(tick));
            assert!(
                snapshot.is_some(),
                "get({tick}) should return Some for ticks in the buffer"
            );
            assert!(
                snapshot.unwrap().pressed(&Action::Jump),
                "Jump should be pressed at tick {tick}"
            );
        }

        // get() for ticks BEYOND the buffer should return None (this is the
        // late-join gap: the client's current tick is often beyond end_tick)
        assert!(
            receiver_buffer.get(Tick(101)).is_none(),
            "get(101) should return None — beyond buffer"
        );

        // get_predict() should return last value for ticks beyond buffer
        let predicted = receiver_buffer.get_predict(Tick(105));
        assert!(predicted.is_some(), "get_predict(105) should return Some");
        assert!(
            predicted.unwrap().pressed(&Action::Jump),
            "predicted should have Jump pressed"
        );
    }

    /// Simulates multiple rebroadcast messages arriving in the same frame
    /// (as happens in practice with redundant input sending).
    /// After processing all messages, get_last() should return the pressed state.
    #[test]
    fn test_multiple_rebroadcast_messages() {
        let mut sender_buffer = InputBuffer::default();
        let mut action_state = ActionState::<Action>::default();
        action_state.press(&Action::Jump);
        for tick in 470..=490 {
            sender_buffer.set(Tick(tick), LeafwingSnapshot(action_state.clone()));
        }

        let mut receiver_buffer: InputBuffer<LeafwingSnapshot<Action>, Action> =
            InputBuffer::default();

        // Simulate 3 messages arriving with different end_ticks (as the server
        // forwards each client packet)
        for end_tick in [485u32, 487, 490] {
            let sequence = LeafwingSequence::<Action>::build_from_input_buffer(
                &sender_buffer,
                20,
                Tick(end_tick),
            )
            .unwrap();
            sequence.update_buffer(&mut receiver_buffer, Tick(end_tick), Duration::default());
        }

        // Buffer should cover through tick 490
        assert_eq!(receiver_buffer.end_tick(), Some(Tick(490)));

        // get_last should have Jump pressed
        let last = receiver_buffer
            .get_last()
            .expect("buffer should not be empty");
        assert!(
            last.pressed(&Action::Jump),
            "get_last should return Jump pressed after multiple messages, got: {:?}",
            last.get_pressed()
        );

        // Check specific ticks
        for tick in 471..=490 {
            let snapshot = receiver_buffer.get(Tick(tick));
            assert!(
                snapshot.is_some_and(|s| s.pressed(&Action::Jump)),
                "Jump should be pressed at tick {tick}"
            );
        }

        // Extending the buffer (as extend_input_buffers_for_late_join does)
        // should preserve the pressed state
        if let Some(last_val) = receiver_buffer.get_last().cloned() {
            receiver_buffer.set(Tick(496), last_val);
        }
        let extended = receiver_buffer
            .get(Tick(496))
            .expect("tick 496 should exist");
        assert!(
            extended.pressed(&Action::Jump),
            "After extension, tick 496 should have Jump pressed, got: {:?}",
            extended.get_pressed()
        );
    }

    /// Check that if the input buffer has ticks beyond the previous mismatch, we still find the mismatch correctly
    #[test]
    fn test_update_buffer_mismatch() {
        let mut input_buffer = InputBuffer::default();
        let mut action_state = ActionState::<Action>::default();
        action_state.press(&Action::Jump);
        input_buffer.set(Tick(2), action_state.clone().into());
        input_buffer.last_remote_tick = Some(Tick(2));
        input_buffer.set(Tick(3), action_state.clone().into());
        input_buffer.set(Tick(4), action_state.clone().into());

        let sequence = LeafwingSequence::<Action> {
            start_state: action_state.clone(),
            diffs: vec![vec![ActionDiff::Released {
                action: Action::Jump,
            }]],
        };
        // Should overwrite tick 3
        sequence.update_buffer(&mut input_buffer, Tick(3), Duration::default());
        assert_eq!(input_buffer.get(Tick(2)).unwrap().0, action_state);

        let mut expected = action_state.clone();
        expected.release(&Action::Jump);
        expected.set_update_state_from_state();
        expected.set_fixed_update_state_from_state();
        assert_eq!(input_buffer.get(Tick(3)).unwrap().0, expected);
    }
}
