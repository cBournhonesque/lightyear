use crate::marker::InputMarker;
use alloc::vec::Vec;
use bevy_ecs::entity::{EntityMapper, MapEntities};
use bevy_ecs::query::QueryData;
use bevy_enhanced_input::action::ActionTime;
use bevy_enhanced_input::prelude::{ActionEvents, ActionState, ActionValue};
use core::fmt::{Debug, Formatter};
use core::time::Duration;
use lightyear_core::prelude::Tick;
use lightyear_inputs::input_buffer::{Compressed, InputBuffer};
use lightyear_inputs::input_message::{ActionStateQueryData, ActionStateSequence, InputSnapshot};
use serde::{Deserialize, Serialize};

pub type BEIBuffer<C> = InputBuffer<ActionsSnapshot, C>;

/// Message containing BEI inputs
#[derive(Serialize, Deserialize)]
pub struct BEIStateSequence<C> {
    start_state: ActionsSnapshot,
    diffs: Vec<Compressed<ActionsDiff>>,
    marker: core::marker::PhantomData<C>,
}

impl<C> PartialEq for BEIStateSequence<C> {
    fn eq(&self, other: &Self) -> bool {
        self.start_state == other.start_state && self.diffs == other.diffs
    }
}

impl<C> Debug for BEIStateSequence<C> {
    fn fmt(&self, f: &mut Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("BEIStateSequence")
            .field("start_state", &self.start_state)
            .field("diffs", &self.diffs)
            .finish()
    }
}

impl<C> Clone for BEIStateSequence<C> {
    fn clone(&self) -> Self {
        Self {
            start_state: self.start_state,
            diffs: self.diffs.clone(),
            marker: core::marker::PhantomData,
        }
    }
}

impl<C> MapEntities for BEIStateSequence<C> {
    fn map_entities<E: EntityMapper>(&mut self, entity_mapper: &mut E) {}
}

/// Struct that stores a subset of [`Actions<C>`](bevy_enhanced_input::prelude::Actions) that is needed to
/// reconstruct the actions state on the remote client.
///
/// We need the timing information in the snapshot so that we can rollback the actions state on the client
/// to a previous state with accurate timing information, or when we fetch a previous actions state if
/// input_delay is enabled
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug)]
pub struct ActionsSnapshot {
    pub state: ActionState,
    pub value: ActionValue,
    pub time: ActionTime,
    pub events: ActionEvents,
}

impl Default for ActionsSnapshot {
    fn default() -> Self {
        Self {
            state: ActionState::default(),
            value: ActionValue::Bool(false),
            time: ActionTime::default(),
            events: ActionEvents::empty(),
        }
    }
}

/// For the diff we only need the State and Value because the events and time can be computed
/// by calling `decay_ticks` from the previous ActionsMessage
#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize)]
struct ActionsDiff {
    state: ActionState,
    value: ActionValue,
}

impl InputSnapshot for ActionsSnapshot {
    fn decay_tick(&mut self, tick_duration: Duration) {
        // We keep ActionState the same but update ActionEvents and ActionTime
        self.events = ActionEvents::new(self.state, self.state);
        let delta_secs = tick_duration.as_secs_f32();
        self.time.update(delta_secs, self.state);
    }
}

#[derive(QueryData, Debug)]
#[query_data(mutable)]
pub struct ActionData {
    state: &'static mut ActionState,
    value: &'static mut ActionValue,
    events: &'static mut ActionEvents,
    time: &'static mut ActionTime,
}

#[derive(Debug)]
pub struct ActionDataInnerItem<'w> {
    pub state: &'w mut ActionState,
    pub value: &'w mut ActionValue,
    pub events: &'w mut ActionEvents,
    pub time: &'w mut ActionTime,
}

impl ActionStateQueryData for ActionData {
    type Mut = ActionData;

    type MutItemInner<'w> = ActionDataInnerItem<'w>;

    type Main = ActionState;
    type Bundle = (ActionState, ActionValue, ActionEvents, ActionTime);

    #[inline]
    fn as_read_only<'a, 'w: 'a, 's>(
        state: &'a <Self::Mut as QueryData>::Item<'w, 's>,
    ) -> <<Self::Mut as QueryData>::ReadOnly as QueryData>::Item<'a, 's> {
        ActionDataReadOnlyItem {
            state: &state.state,
            value: &state.value,
            events: &state.events,
            time: &state.time,
        }
    }

    #[inline]
    fn into_inner<'w, 's>(
        mut_item: <Self::Mut as QueryData>::Item<'w, 's>,
    ) -> Self::MutItemInner<'w> {
        ActionDataInnerItem {
            state: mut_item.state.into_inner(),
            value: mut_item.value.into_inner(),
            events: mut_item.events.into_inner(),
            time: mut_item.time.into_inner(),
        }
    }

    #[inline]
    fn as_mut(bundle: &mut Self::Bundle) -> Self::MutItemInner<'_> {
        let (state, value, events, time) = bundle;
        ActionDataInnerItem {
            state,
            value,
            events,
            time,
        }
    }

    #[inline]
    fn base_value() -> Self::Bundle {
        (
            ActionState::default(),
            ActionValue::Bool(false),
            ActionEvents::empty(),
            ActionTime::default(),
        )
    }
}

impl<C: Send + Sync + 'static> ActionStateSequence for BEIStateSequence<C> {
    type Action = C;
    type Snapshot = ActionsSnapshot;
    type State = ActionData;
    type Marker = InputMarker<C>;

    fn len(&self) -> usize {
        self.diffs.len() + 1
    }

    fn get_snapshots_from_message(
        self,
        tick_duration: Duration,
    ) -> impl Iterator<Item = Compressed<Self::Snapshot>> {
        let start_iter = core::iter::once(Compressed::Input(self.start_state));
        let diffs_iter = self.diffs.into_iter().scan(
            self.start_state,
            move |state: &mut ActionsSnapshot, diff: Compressed<ActionsDiff>| {
                let (new_state, new_value) = match diff {
                    Compressed::Absent => return Some(Compressed::Absent),
                    Compressed::SameAsPrecedent => (state.state, state.value),
                    Compressed::Input(diff) => (diff.state, diff.value),
                };
                state.events = ActionEvents::new(state.state, new_state);
                let delta_secs = tick_duration.as_secs_f32();
                state.time.update(delta_secs, state.state);
                state.state = new_state;
                state.value = new_value;

                // TODO: should we output SameAsPrecedent if the state did not change? Or is there no point
                //  in compressing the InputBuffer?
                Some(Compressed::Input(*state))
            },
        );
        start_iter.chain(diffs_iter)
    }

    fn build_from_input_buffer<'w, 's>(
        input_buffer: &InputBuffer<Self::Snapshot, Self::Action>,
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
        let start_state = *input_buffer.get(start_tick).unwrap();
        let mut tick = start_tick + 1;
        let (mut cur_state, mut cur_value) = (start_state.state, start_state.value);
        while tick <= end_tick {
            let diff = match input_buffer.get_raw(tick) {
                Compressed::Absent => Compressed::Absent,
                Compressed::SameAsPrecedent => Compressed::SameAsPrecedent,
                Compressed::Input(snapshot) => {
                    let diff = if snapshot.state == cur_state && snapshot.value == cur_value {
                        Compressed::SameAsPrecedent
                    } else {
                        Compressed::Input(ActionsDiff {
                            state: snapshot.state,
                            value: snapshot.value,
                        })
                    };
                    cur_state = snapshot.state;
                    cur_value = snapshot.value;
                    diff
                }
            };
            diffs.push(diff);
            tick += 1;
        }
        Some(Self {
            start_state,
            diffs,
            marker: core::marker::PhantomData,
        })
    }

    fn to_snapshot<'w, 's>(state: ActionDataReadOnlyItem) -> Self::Snapshot {
        ActionsSnapshot {
            state: *state.state,
            value: *state.value,
            events: *state.events,
            time: *state.time,
        }
    }

    fn from_snapshot<'w, 's>(state: ActionDataInnerItem, snapshot: &Self::Snapshot) {
        *state.state = snapshot.state;
        *state.value = snapshot.value;
        *state.events = snapshot.events;
        *state.time = snapshot.time;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use core::time::Duration;

    use bevy_enhanced_input::prelude::InputAction;
    use bevy_reflect::Reflect;
    use test_log::test;
    use tracing::info;

    struct Context1;

    #[derive(InputAction, Debug, Clone, PartialEq, Eq, Hash, Reflect)]
    #[action_output(bool)]
    struct Action1;

    #[test]
    fn test_create_message() {
        let mut input_buffer = BEIBuffer::<Context1>::default();
        let mut state = ActionsSnapshot::default();

        input_buffer.set(Tick(2), state);
        state.state = ActionState::Fired;
        state.value = ActionValue::Bool(true);
        input_buffer.set(Tick(3), state);
        state.state = ActionState::None;
        state.value = ActionValue::Bool(false);
        input_buffer.set(Tick(7), state);

        let sequence =
            BEIStateSequence::<Context1>::build_from_input_buffer(&input_buffer, 9, Tick(10))
                .unwrap();
        assert_eq!(
            sequence,
            BEIStateSequence::<Context1> {
                // tick 2
                start_state: ActionsSnapshot {
                    state: ActionState::None,
                    value: ActionValue::Bool(false),
                    events: ActionEvents::empty(),
                    time: ActionTime::default(),
                },
                diffs: vec![
                    Compressed::Input(ActionsDiff {
                        state: ActionState::Fired,
                        value: ActionValue::Bool(true)
                    }),
                    Compressed::SameAsPrecedent,
                    Compressed::SameAsPrecedent,
                    Compressed::SameAsPrecedent,
                    Compressed::Input(ActionsDiff {
                        state: ActionState::None,
                        value: ActionValue::Bool(false)
                    }),
                    Compressed::Absent,
                    Compressed::Absent,
                    Compressed::Absent,
                ],
                marker: Default::default(),
            }
        );
    }

    #[test]
    fn test_build_from_input_buffer_empty() {
        let input_buffer: BEIBuffer<Context1> = InputBuffer::default();
        let sequence =
            BEIStateSequence::<Context1>::build_from_input_buffer(&input_buffer, 5, Tick(10));
        assert!(sequence.is_none());
    }

    #[test]
    fn test_build_from_input_buffer_partial_overlap() {
        let mut input_buffer = BEIBuffer::<Context1>::default();
        let mut state = ActionsSnapshot::default();
        input_buffer.set(Tick(8), state);
        state.state = ActionState::Fired;
        state.value = ActionValue::Bool(true);
        input_buffer.set(Tick(10), state);

        let sequence =
            BEIStateSequence::<Context1>::build_from_input_buffer(&input_buffer, 5, Tick(12))
                .unwrap();
        assert_eq!(sequence.len(), 5);
    }

    #[test]
    fn test_update_buffer_extends_left_and_right() {
        let mut input_buffer = BEIBuffer::<Context1>::default();
        let state = ActionsSnapshot::default();
        let sequence = BEIStateSequence::<Context1> {
            start_state: state,
            diffs: vec![Compressed::SameAsPrecedent, Compressed::Absent],
            marker: Default::default(),
        };
        // This should extend the buffer to fit ticks 5..=7
        sequence.update_buffer(&mut input_buffer, Tick(7), Duration::default());
        assert!(input_buffer.get(Tick(5)).is_some());
        assert!(input_buffer.get(Tick(6)).is_some());
        assert!(input_buffer.get(Tick(7)).is_none());
    }

    #[test]
    fn test_update_buffer_empty_buffer() {
        let mut input_buffer = BEIBuffer::<Context1>::default();
        let mut state = ActionsSnapshot::default();
        state.state = ActionState::Fired;
        state.value = ActionValue::Bool(true);
        let sequence = BEIStateSequence::<Context1> {
            start_state: state,
            diffs: vec![Compressed::SameAsPrecedent, Compressed::Absent],
            marker: Default::default(),
        };

        let earliest_mismatch =
            sequence.update_buffer(&mut input_buffer, Tick(7), Duration::default());

        info!("Input buffer after update: {:?}", input_buffer);

        // With empty buffer, the first tick received is a mismatch
        assert_eq!(earliest_mismatch, Some(Tick(5)));
        assert_eq!(input_buffer.start_tick, Some(Tick(5)));
        assert_eq!(input_buffer.get(Tick(5)), Some(&state));
        state.events = ActionEvents::FIRED;
        assert_eq!(input_buffer.get(Tick(6)), Some(&state));
        assert_eq!(input_buffer.get(Tick(7)), None);
    }

    #[test]
    fn test_update_buffer_last_action_absent_new_action_present() {
        let mut input_buffer = BEIBuffer::<Context1>::default();
        let mut state = ActionsSnapshot::default();

        // Set up buffer with an absent action at tick 5
        input_buffer.set_empty(Tick(5));
        input_buffer.last_remote_tick = Some(Tick(5));

        // Create a new action for the message
        state.state = ActionState::Fired;
        state.value = ActionValue::Bool(true);

        let sequence = BEIStateSequence::<Context1> {
            start_state: state,
            diffs: vec![Compressed::SameAsPrecedent],
            marker: Default::default(),
        };

        let earliest_mismatch =
            sequence.update_buffer(&mut input_buffer, Tick(8), Duration::default());

        // Should detect mismatch at tick 7 (first tick after previous_end_tick=5)
        // We predicted continuation of Absent, but got an Input
        assert_eq!(earliest_mismatch, Some(Tick(7)));
        // Filled the gap with SameAsPrecedent at tick 6, then set the new action at tick 7 and 8
        assert_eq!(input_buffer.get_raw(Tick(6)), &Compressed::SameAsPrecedent);
        assert_eq!(input_buffer.get(Tick(7)), Some(&state));
        state.events = ActionEvents::FIRED;
        assert_eq!(input_buffer.get(Tick(8)), Some(&state));
    }

    #[test]
    fn test_update_buffer_action_mismatch() {
        let mut input_buffer = BEIBuffer::<Context1>::default();

        let mut state = ActionsSnapshot::default();

        // Set up buffer with one action at tick 5
        state.state = ActionState::Fired;
        state.value = ActionValue::Bool(true);
        input_buffer.set(Tick(5), state);
        input_buffer.last_remote_tick = Some(Tick(5));

        // Create a different action for the message
        state.state = ActionState::Ongoing;
        state.value = ActionValue::Bool(false);

        let sequence = BEIStateSequence::<Context1> {
            start_state: state,
            diffs: vec![Compressed::SameAsPrecedent],
            marker: Default::default(),
        };

        let earliest_mismatch =
            sequence.update_buffer(&mut input_buffer, Tick(7), Duration::default());

        // Should detect mismatch at tick 6 (first tick after previous_end_tick=5)
        // We predicted continuation of first_action, but got second_action
        assert_eq!(earliest_mismatch, Some(Tick(6)));
        assert_eq!(input_buffer.get(Tick(6)), Some(&state));
        // check that we decayed the state correctly, the state is now ongoing
        state.events = ActionEvents::ONGOING;
        assert_eq!(input_buffer.get(Tick(7)), Some(&state));
    }

    #[test]
    fn test_update_buffer_no_mismatch_same_action() {
        let mut input_buffer = BEIBuffer::<Context1>::default();

        // Set up buffer with an action at tick 5
        let mut state = ActionsSnapshot::default();
        state.state = ActionState::Fired;
        state.value = ActionValue::Bool(true);
        input_buffer.set(Tick(5), state);
        input_buffer.last_remote_tick = Some(Tick(5));

        // Receive a snapshot for ticks 6 and 7 with the same action (decayed)
        let mut snapshot = state;
        snapshot.decay_tick(Duration::default());
        // the second action doesn't mismatch because there are no durations
        let sequence = BEIStateSequence::<Context1> {
            start_state: snapshot,
            diffs: vec![Compressed::SameAsPrecedent],
            marker: Default::default(),
        };

        let earliest_mismatch =
            sequence.update_buffer(&mut input_buffer, Tick(7), Duration::default());

        // Should be no mismatch since the action matches our prediction
        assert_eq!(earliest_mismatch, None);
        assert_eq!(
            input_buffer.get_raw(Tick(6)),
            &Compressed::Input(snapshot.clone())
        );
        snapshot.decay_tick(Duration::default());
        assert_eq!(input_buffer.get(Tick(7)), Some(&snapshot));
        assert_eq!(input_buffer.get_raw(Tick(8)), &Compressed::Absent);
    }

    #[test]
    fn test_update_buffer_skip_ticks_before_previous_end() {
        let mut input_buffer = BEIBuffer::<Context1>::default();

        // Set up buffer with actions at ticks 5 and 6
        let mut state = ActionsSnapshot::default();
        state.state = ActionState::Fired;
        state.value = ActionValue::Bool(true);
        let first_state = state;
        input_buffer.set(Tick(5), first_state);
        input_buffer.set(Tick(6), first_state);
        input_buffer.last_remote_tick = Some(Tick(6));

        // Create a different action
        state.state = ActionState::Ongoing;
        state.value = ActionValue::Bool(false);
        let mut second_state = state;

        // Message covers ticks 5-8, but we should only process ticks 7-8
        let sequence = BEIStateSequence::<Context1> {
            start_state: first_state,
            diffs: vec![
                Compressed::SameAsPrecedent, // tick 6 - should be skipped
                Compressed::Input(ActionsDiff {
                    state: second_state.state,
                    value: second_state.value,
                }), // tick 7 - should detect mismatch
                Compressed::SameAsPrecedent, // tick 8 - should be processed
            ],
            marker: Default::default(),
        };

        let earliest_mismatch =
            sequence.update_buffer(&mut input_buffer, Tick(8), Duration::default());

        // Should detect mismatch at tick 7 (first tick after previous_end_tick=6)
        assert_eq!(earliest_mismatch, Some(Tick(7)));
        // Ticks 5 and 6 should remain unchanged
        assert_eq!(input_buffer.get(Tick(5)), Some(&first_state));
        assert_eq!(input_buffer.get(Tick(6)), Some(&first_state));
        // Ticks 7 and 8 should be updated
        second_state.events = ActionEvents::ONGOING;
        assert_eq!(input_buffer.get(Tick(7)), Some(&second_state));
        assert_eq!(input_buffer.get(Tick(8)), Some(&second_state));
    }

    /// Even if last_remote_tick < end_tick, we should correctly compute the mismatch
    #[test]
    fn test_update_buffer_last_remote_tick_before_end_tick() {
        let mut input_buffer = BEIBuffer::default();

        // Set up buffer with actions at ticks 5 and 6
        let mut state = ActionsSnapshot::default();
        state.state = ActionState::Fired;
        state.value = ActionValue::Bool(true);

        let first_state = state;
        input_buffer.set(Tick(6), first_state);

        let mut first_state_decay = first_state;
        first_state_decay.decay_tick(Duration::from_millis(10));
        input_buffer.set(Tick(7), first_state_decay);
        input_buffer.last_remote_tick = Some(Tick(6));

        info!("Input buffer before update: {:?}", input_buffer);

        let sequence = BEIStateSequence::<Context1> {
            start_state: first_state,
            diffs: vec![
                Compressed::SameAsPrecedent, // tick 7 - should be mismatch because it's not decayed!
            ],
            marker: Default::default(),
        };

        let earliest_mismatch =
            sequence.update_buffer(&mut input_buffer, Tick(7), Duration::default());

        // Should detect mismatch at tick 7 (first tick after previous_end_tick=6)
        assert_eq!(earliest_mismatch, Some(Tick(7)));
    }
}
