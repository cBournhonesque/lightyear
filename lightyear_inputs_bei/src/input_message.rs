use crate::marker::InputMarker;
use alloc::{vec, vec::Vec};
use bevy_ecs::entity::{EntityMapper, MapEntities};
use bevy_ecs::query::QueryData;
use bevy_enhanced_input::action::ActionTime;
use bevy_enhanced_input::prelude::{ActionEvents, ActionState, ActionValue};
use core::cmp::max;
use core::fmt::{Debug, Formatter};
use core::time::Duration;
use lightyear_core::prelude::Tick;
use lightyear_inputs::input_buffer::{InputBuffer, InputData};
use lightyear_inputs::input_message::{ActionStateQueryData, ActionStateSequence, InputSnapshot};
use serde::{Deserialize, Serialize};

pub type SnapshotBuffer<A> = InputBuffer<ActionsSnapshot<A>>;

pub struct BEIStateSequence<C> {
    // TODO: use InputData for each action separately to optimize the diffs
    states: Vec<InputData<ActionsMessage>>,
    marker: core::marker::PhantomData<C>,
}

impl<C> Serialize for BEIStateSequence<C> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.states.serialize(serializer)
    }
}

impl<'de, C> Deserialize<'de> for BEIStateSequence<C> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let states = Vec::<InputData<ActionsMessage>>::deserialize(deserializer)?;
        Ok(Self {
            states,
            marker: core::marker::PhantomData,
        })
    }
}

impl<C> PartialEq for BEIStateSequence<C> {
    fn eq(&self, other: &Self) -> bool {
        self.states == other.states
    }
}

impl<C> Debug for BEIStateSequence<C> {
    fn fmt(&self, f: &mut Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("BEIStateSequence")
            .field("states", &self.states)
            .finish()
    }
}

impl<C> Clone for BEIStateSequence<C> {
    fn clone(&self) -> Self {
        Self {
            states: self.states.clone(),
            marker: core::marker::PhantomData,
        }
    }
}

impl<C> MapEntities for BEIStateSequence<C> {
    fn map_entities<E: EntityMapper>(&mut self, entity_mapper: &mut E) {}
}

/// Instead of replicating the BEI Actions, we will replicate a serializable subset that can be used to
/// fully know on the remote client which actions should be triggered. This data will be used
/// to update the BEI `Actions` component
#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize)]
struct ActionsMessage {
    state: ActionState,
    value: ActionValue,
    time: ActionTime,
    events: ActionEvents,
}

impl Default for ActionsMessage {
    fn default() -> Self {
        Self {
            state: ActionState::default(),
            value: ActionValue::Bool(false),
            time: ActionTime::default(),
            events: ActionEvents::empty(),
        }
    }
}

/// Struct that stores a subset of [`Actions<C>`](bevy_enhanced_input::prelude::Actions) that is needed to
/// reconstruct the actions state on the remote client.
///
/// We need the timing information in the snapshot so that we can rollback the actions state on the client
/// to a previous state with accurate timing information, or when we fetch a previous actions state if
/// input_delay is enabled
pub struct ActionsSnapshot<C> {
    state: ActionsMessage,
    _marker: core::marker::PhantomData<C>,
}

impl<C> ActionsSnapshot<C> {
    pub fn new(
        state: ActionState,
        value: ActionValue,
        time: ActionTime,
        events: ActionEvents,
    ) -> Self {
        Self {
            state: ActionsMessage {
                state,
                value,
                time,
                events,
            },
            _marker: core::marker::PhantomData,
        }
    }
}

impl<C> Default for ActionsSnapshot<C> {
    fn default() -> Self {
        Self {
            state: ActionsMessage::default(),
            _marker: core::marker::PhantomData,
        }
    }
}

impl<C> Clone for ActionsSnapshot<C> {
    fn clone(&self) -> Self {
        Self {
            state: self.state,
            _marker: core::marker::PhantomData,
        }
    }
}

impl<C> PartialEq for ActionsSnapshot<C> {
    fn eq(&self, other: &Self) -> bool {
        self.state.eq(&other.state)
    }
}

impl<C> Debug for ActionsSnapshot<C> {
    fn fmt(&self, f: &mut Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ActionsSnapshot")
            .field("state", &self.state)
            .finish()
    }
}

impl<C: Send + Sync + 'static> InputSnapshot for ActionsSnapshot<C> {
    type Action = C;
    fn decay_tick(&mut self, tick_duration: Duration) {
        // We keep ActionState the same but update ActionEvents and ActionTime
        self.state.events = ActionEvents::new(self.state.state, self.state.state);
        // TODO: use self.state.time.update() when it's public
        let delta_secs = tick_duration.as_secs_f32();
        match self.state.state {
            ActionState::None => {
                self.state.time.elapsed_secs = 0.0;
                self.state.time.fired_secs = 0.0;
            }
            ActionState::Ongoing => {
                self.state.time.elapsed_secs += delta_secs;
                self.state.time.fired_secs = 0.0;
            }
            ActionState::Fired => {
                self.state.time.elapsed_secs += delta_secs;
                self.state.time.fired_secs += delta_secs;
            }
        }
    }
}

impl ActionsMessage {
    fn from_snapshot<C>(snapshot: &ActionsSnapshot<C>) -> Self {
        snapshot.state
    }

    #[allow(clippy::wrong_self_convention)]
    fn to_snapshot<C>(self) -> ActionsSnapshot<C> {
        ActionsSnapshot::<C> {
            state: self,
            _marker: core::marker::PhantomData,
        }
    }
}

#[derive(QueryData)]
#[query_data(mutable)]
pub struct ActionData {
    state: &'static mut ActionState,
    value: &'static mut ActionValue,
    events: &'static mut ActionEvents,
    time: &'static mut ActionTime,
}

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

    fn as_read_only<'a, 'w: 'a>(state: &'a ActionDataItem<'w>) -> ActionDataReadOnlyItem<'a> {
        ActionDataReadOnlyItem {
            state: &state.state,
            value: &state.value,
            events: &state.events,
            time: &state.time,
        }
    }

    fn into_inner<'w>(mut_item: <Self::Mut as QueryData>::Item<'w>) -> Self::MutItemInner<'w> {
        ActionDataInnerItem {
            state: mut_item.state.into_inner(),
            value: mut_item.value.into_inner(),
            events: mut_item.events.into_inner(),
            time: mut_item.time.into_inner(),
        }
    }

    fn as_mut<'w>(bundle: &'w mut Self::Bundle) -> Self::MutItemInner<'w> {
        let (state, value, events, time) = bundle;
        ActionDataInnerItem {
            state,
            value,
            events,
            time,
        }
    }

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
    type Snapshot = ActionsSnapshot<C>;
    type State = ActionData;
    type Marker = InputMarker<C>;

    fn is_empty(&self) -> bool {
        self.states.is_empty()
            || self
                .states
                .iter()
                // TODO: is this correct? we could have all SameAsPrecedent but similar to something non-empty?
                .all(|s| matches!(s, InputData::Absent | InputData::SameAsPrecedent))
    }

    fn len(&self) -> usize {
        self.states.len()
    }

    fn get_snapshots_from_message(self) -> impl Iterator<Item = InputData<Self::Snapshot>> {
        self.states.into_iter().map(|input| match input {
            InputData::Absent => InputData::Absent,
            InputData::SameAsPrecedent => InputData::SameAsPrecedent,
            InputData::Input(i) => InputData::Input(i.to_snapshot()),
        })
    }

    fn build_from_input_buffer<'w, 's>(
        input_buffer: &InputBuffer<Self::Snapshot>,
        num_ticks: u16,
        end_tick: Tick,
    ) -> Option<Self> {
        let buffer_start_tick = input_buffer.start_tick?;
        // find the first tick for which we have an `ActionState` buffered
        let start_tick = max(end_tick - num_ticks + 1, buffer_start_tick);

        // find the initial state, (which we convert out of SameAsPrecedent)
        let start_state = input_buffer
            .get(start_tick)
            .map_or(InputData::Absent, |input| {
                InputData::Input(ActionsMessage::from_snapshot(input))
            });
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
                        InputData::Input(v) => InputData::Input(ActionsMessage::from_snapshot(v)),
                    });
            states.push(state);
        }
        Some(Self {
            states,
            marker: core::marker::PhantomData,
        })
    }

    fn to_snapshot<'w, 's>(state: ActionDataReadOnlyItem) -> Self::Snapshot {
        ActionsSnapshot {
            state: ActionsMessage {
                state: *state.state,
                value: *state.value,
                events: *state.events,
                time: *state.time,
            },
            _marker: core::marker::PhantomData,
        }
    }

    fn from_snapshot<'w, 's>(state: ActionDataInnerItem, snapshot: &Self::Snapshot) {
        let snapshot = &snapshot.state;
        *state.state = snapshot.state;
        *state.value = snapshot.value;
        *state.events = snapshot.events;
        *state.time = snapshot.time;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

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
        let mut input_buffer = InputBuffer::default();
        let mut state = ActionsMessage::default();

        input_buffer.set(
            Tick(2),
            ActionsSnapshot::<Context1> {
                state,
                _marker: Default::default(),
            },
        );
        state.state = ActionState::Fired;
        state.value = ActionValue::Bool(true);
        input_buffer.set(
            Tick(3),
            ActionsSnapshot::<Context1> {
                state,
                _marker: Default::default(),
            },
        );
        state.state = ActionState::None;
        state.value = ActionValue::Bool(false);
        input_buffer.set(
            Tick(7),
            ActionsSnapshot::<Context1> {
                state,
                _marker: Default::default(),
            },
        );

        let sequence =
            BEIStateSequence::<Context1>::build_from_input_buffer(&input_buffer, 9, Tick(10))
                .unwrap();
        assert_eq!(
            sequence,
            BEIStateSequence::<Context1> {
                // tick 2
                states: vec![
                    InputData::Input(ActionsMessage {
                        state: ActionState::None,
                        value: ActionValue::Bool(false),
                        events: ActionEvents::empty(),
                        time: ActionTime::default(),
                    }),
                    InputData::Input(ActionsMessage {
                        state: ActionState::Fired,
                        value: ActionValue::Bool(true),
                        events: ActionEvents::empty(),
                        time: ActionTime::default(),
                    }),
                    InputData::SameAsPrecedent,
                    InputData::SameAsPrecedent,
                    InputData::SameAsPrecedent,
                    InputData::Input(ActionsMessage {
                        state: ActionState::None,
                        value: ActionValue::Bool(false),
                        events: ActionEvents::empty(),
                        time: ActionTime::default(),
                    }),
                    InputData::Absent,
                    InputData::Absent,
                    InputData::Absent,
                ],
                marker: Default::default(),
            }
        );
    }

    #[test]
    fn test_build_from_input_buffer_empty() {
        let input_buffer: InputBuffer<ActionsSnapshot<Context1>> = InputBuffer::default();
        let sequence =
            BEIStateSequence::<Context1>::build_from_input_buffer(&input_buffer, 5, Tick(10));
        assert!(sequence.is_none());
    }

    #[test]
    fn test_build_from_input_buffer_partial_overlap() {
        let mut input_buffer = InputBuffer::default();
        let mut state = ActionsMessage::default();
        input_buffer.set(
            Tick(8),
            ActionsSnapshot::<Context1> {
                state,
                _marker: Default::default(),
            },
        );
        state.state = ActionState::Fired;
        state.value = ActionValue::Bool(true);
        input_buffer.set(
            Tick(10),
            ActionsSnapshot::<Context1> {
                state,
                _marker: Default::default(),
            },
        );

        let sequence =
            BEIStateSequence::<Context1>::build_from_input_buffer(&input_buffer, 5, Tick(12))
                .unwrap();
        assert!(matches!(&sequence.states[0], InputData::Input(_)));
        assert_eq!(sequence.states.len(), 5);
    }

    #[test]
    fn test_update_buffer_extends_left_and_right() {
        let mut input_buffer = InputBuffer::default();
        let actions_msg = ActionsMessage::default();
        let sequence = BEIStateSequence::<Context1> {
            states: vec![
                InputData::Input(actions_msg),
                InputData::SameAsPrecedent,
                InputData::Absent,
            ],
            marker: Default::default(),
        };
        // This should extend the buffer to fit ticks 5..=7
        sequence.update_buffer(
            &mut input_buffer,
            Tick(7),
            TickDuration(Duration::default()),
        );
        assert!(input_buffer.get(Tick(5)).is_some());
        assert!(input_buffer.get(Tick(6)).is_some());
        assert!(input_buffer.get(Tick(7)).is_none());
    }

    #[test]
    fn test_update_buffer_empty_buffer() {
        let mut input_buffer = InputBuffer::default();
        let mut state = ActionsMessage::default();
        state.state = ActionState::Fired;
        state.value = ActionValue::Bool(true);
        let sequence = BEIStateSequence::<Context1> {
            states: vec![
                InputData::Input(state),
                InputData::SameAsPrecedent,
                InputData::Absent,
            ],
            marker: Default::default(),
        };

        let earliest_mismatch = sequence.update_buffer(
            &mut input_buffer,
            Tick(7),
            TickDuration(Duration::default()),
        );

        info!("Input buffer after update: {:?}", input_buffer);

        // With empty buffer, the first tick received is a mismatch
        assert_eq!(earliest_mismatch, Some(Tick(5)));
        assert_eq!(input_buffer.start_tick, Some(Tick(5)));
        assert_eq!(input_buffer.get(Tick(5)), Some(&state.to_snapshot()));
        assert_eq!(input_buffer.get(Tick(6)), Some(&state.to_snapshot()));
        assert_eq!(input_buffer.get(Tick(7)), None);
    }

    #[test]
    fn test_update_buffer_last_action_absent_new_action_present() {
        let mut input_buffer = InputBuffer::default();
        let mut state = ActionsMessage::default();

        // Set up buffer with an absent action at tick 5
        input_buffer.set_empty(Tick(5));

        // Create a new action for the message
        state.state = ActionState::Fired;
        state.value = ActionValue::Bool(true);

        let sequence = BEIStateSequence::<Context1> {
            states: vec![InputData::Input(state), InputData::SameAsPrecedent],
            marker: Default::default(),
        };

        let earliest_mismatch = sequence.update_buffer(
            &mut input_buffer,
            Tick(8),
            TickDuration(Duration::default()),
        );

        // Should detect mismatch at tick 7 (first tick after previous_end_tick=5)
        // We predicted continuation of Absent, but got an Input
        assert_eq!(earliest_mismatch, Some(Tick(7)));
        // Filled the gap with SameAsPrecedent at tick 6, then set the new action at tick 7 and 8
        assert_eq!(input_buffer.get_raw(Tick(6)), &InputData::SameAsPrecedent);
        assert_eq!(input_buffer.get(Tick(7)), Some(&state.to_snapshot()));
        assert_eq!(input_buffer.get(Tick(8)), Some(&state.to_snapshot()));
    }

    #[test]
    fn test_update_buffer_last_action_present_new_action_absent() {
        let mut input_buffer = InputBuffer::default();

        // Set up buffer with a present action at tick 5
        let mut state = ActionsMessage::default();
        state.state = ActionState::Fired;
        state.value = ActionValue::Bool(true);
        input_buffer.set(Tick(5), state.to_snapshot());

        let sequence = BEIStateSequence::<Context1> {
            states: vec![InputData::Absent, InputData::SameAsPrecedent],
            marker: Default::default(),
        };

        let earliest_mismatch = sequence.update_buffer(
            &mut input_buffer,
            Tick(7),
            TickDuration(Duration::default()),
        );

        // Should detect mismatch at tick 6 (first tick after previous_end_tick=5)
        // We predicted continuation of the action, but got Absent
        assert_eq!(earliest_mismatch, Some(Tick(6)));
        assert_eq!(input_buffer.get(Tick(6)), None);
        assert_eq!(input_buffer.get(Tick(7)), None);
    }

    #[test]
    fn test_update_buffer_action_mismatch() {
        let mut input_buffer = InputBuffer::default();

        let mut state = ActionsMessage::default();

        // Set up buffer with one action at tick 5
        state.state = ActionState::Fired;
        state.value = ActionValue::Bool(true);
        input_buffer.set(Tick(5), state.to_snapshot());

        // Create a different action for the message
        state.state = ActionState::Ongoing;
        state.value = ActionValue::Bool(false);

        let sequence = BEIStateSequence::<Context1> {
            states: vec![InputData::Input(state), InputData::SameAsPrecedent],
            marker: Default::default(),
        };

        let earliest_mismatch = sequence.update_buffer(
            &mut input_buffer,
            Tick(7),
            TickDuration(Duration::default()),
        );

        // Should detect mismatch at tick 6 (first tick after previous_end_tick=5)
        // We predicted continuation of first_action, but got second_action
        assert_eq!(earliest_mismatch, Some(Tick(6)));
        assert_eq!(input_buffer.get(Tick(6)), Some(&state.to_snapshot()));
        assert_eq!(input_buffer.get(Tick(7)), Some(&state.to_snapshot()));
    }

    #[test]
    fn test_update_buffer_no_mismatch_same_action() {
        let mut input_buffer = InputBuffer::default();

        // Set up buffer with an action at tick 5
        let mut state = ActionsMessage::default();
        state.state = ActionState::Fired;
        state.value = ActionValue::Bool(true);
        input_buffer.set(Tick(5), state.to_snapshot());

        let sequence = BEIStateSequence::<Context1> {
            states: vec![InputData::Input(state), InputData::SameAsPrecedent],
            marker: Default::default(),
        };

        let earliest_mismatch = sequence.update_buffer(
            &mut input_buffer,
            Tick(7),
            TickDuration(Duration::default()),
        );

        // Should be no mismatch since the action matches our prediction
        assert_eq!(earliest_mismatch, None);
        assert_eq!(input_buffer.get_raw(Tick(6)), &InputData::SameAsPrecedent);
        assert_eq!(input_buffer.get_raw(Tick(7)), &InputData::SameAsPrecedent);
        assert_eq!(input_buffer.get_raw(Tick(8)), &InputData::Absent);
    }

    #[test]
    fn test_update_buffer_skip_ticks_before_previous_end() {
        let mut input_buffer = InputBuffer::default();

        // Set up buffer with actions at ticks 5 and 6
        let mut state = ActionsMessage::default();
        state.state = ActionState::Fired;
        state.value = ActionValue::Bool(true);
        let first_state = state;
        input_buffer.set(Tick(5), first_state.to_snapshot());
        input_buffer.set(Tick(6), first_state.to_snapshot());

        // Create a different action
        state.state = ActionState::Ongoing;
        state.value = ActionValue::Bool(false);
        let second_state = state;

        // Message covers ticks 5-8, but we should only process ticks 7-8
        let sequence = BEIStateSequence::<Context1> {
            states: vec![
                InputData::Input(first_state),  // tick 5 - should be skipped
                InputData::SameAsPrecedent,     // tick 6 - should be skipped
                InputData::Input(second_state), // tick 7 - should detect mismatch
                InputData::SameAsPrecedent,     // tick 8 - should be processed
            ],
            marker: Default::default(),
        };

        let earliest_mismatch = sequence.update_buffer(
            &mut input_buffer,
            Tick(8),
            TickDuration(Duration::default()),
        );

        // Should detect mismatch at tick 7 (first tick after previous_end_tick=6)
        assert_eq!(earliest_mismatch, Some(Tick(7)));
        // Ticks 5 and 6 should remain unchanged
        assert_eq!(input_buffer.get(Tick(5)), Some(&first_state.to_snapshot()));
        assert_eq!(input_buffer.get(Tick(6)), Some(&first_state.to_snapshot()));
        // Ticks 7 and 8 should be updated
        assert_eq!(input_buffer.get(Tick(7)), Some(&second_state.to_snapshot()));
        assert_eq!(input_buffer.get(Tick(8)), Some(&second_state.to_snapshot()));
    }
}
