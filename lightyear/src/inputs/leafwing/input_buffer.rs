use std::collections::VecDeque;
use std::fmt::Debug;

use bevy::prelude::{Component, Entity, Resource, TypePath};
use bevy::utils::{EntityHashMap, EntityHashSet, HashMap};
use leafwing_input_manager::common_conditions::action_just_pressed;
use leafwing_input_manager::prelude::ActionState;
use leafwing_input_manager::Actionlike;
use serde::{Deserialize, Serialize};

use crate::client::components::ComponentSyncMode;
use crate::prelude::client::SyncComponent;
use crate::prelude::{EntityMapper, MapEntities, Message, Named};
use lightyear_macros::MessageInternal;

use super::UserAction;
use crate::protocol::BitSerializable;
use crate::shared::tick_manager::Tick;

// NOTE: we can have multiple Actionlike, (each entity could have a different Actionlike),
//  so we will have a separate InputBuffer for each!

// CLIENT:
// - store the diffs for each past ticks
// - during rollback we can apply the diffs in reverse -> is this possible?
//   - if not possible, we just store the ActionState for each tick (a bit expensive...)
//   - should be ok if we pre-allocate

// SERVER:
// - we receive a message containing for each tick a list of diffs
// - we apply the ticks on the right tick to the entity/resource
// - no need to maintain our inputbuffer on the server

impl<A: UserAction> Message for ActionState<A> {}
impl<'a, A: UserAction> MapEntities<'a> for ActionState<A> {
    fn map_entities(&mut self, entity_mapper: Box<dyn EntityMapper + 'a>) {}
    fn entities(&self) -> EntityHashSet<Entity> {
        EntityHashSet::default()
    }
}
impl<A: UserAction> Named for ActionState<A> {
    fn name(&self) -> &'static str {
        <A as TypePath>::short_type_path()
        // const SHORT_TYPE_PATH: &'static str = <A as TypePath>::short_type_path();
        // formatcp!("ActionState<{}>", SHORT_TYPE_PATH)
    }
}

impl<A: UserAction> SyncComponent for ActionState<A> {
    fn mode() -> ComponentSyncMode {
        ComponentSyncMode::Once
    }
}

// NOTE: right now, for simplicity, we will send all the action-diffs for all entities in one single message.
// TODO: improve this data structure
#[derive(Resource, Component, Debug)]
pub(crate) struct InputBuffer<A: UserAction> {
    start_tick: Option<Tick>,
    buffer: VecDeque<BufferItem<ActionState<A>>>,
}

// We use this to avoid cloning values in the buffer too much
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
enum BufferItem<T> {
    Absent,
    SameAsPrecedent,
    Data(T),
}

impl<A: UserAction> Default for InputBuffer<A> {
    fn default() -> Self {
        Self {
            start_tick: None,
            buffer: VecDeque::new(),
        }
    }
}

/// Whether an action was just pressed or released. We can use this to reconstruct the ActionState
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub enum ActionDiff<A: UserAction> {
    Pressed(A),
    Released(A),
}

impl<A: UserAction> ActionDiff<A> {
    pub(crate) fn apply(self, action_state: &mut ActionState<A>) {
        match self {
            ActionDiff::Pressed(action) => {
                action_state.press(action);
            }
            ActionDiff::Released(action) => {
                action_state.release(action);
            }
        }
    }
}

// TODO: use Mode to specify how to serialize a message (serde vs bitcode)! + can specify custom serialize function as well (similar to interpolation mode)
#[derive(MessageInternal, Serialize, Deserialize, Clone, PartialEq, Debug)]
#[message(custom_map)]
/// We serialize the inputs by sending only the ActionDiffs of the last few ticks
/// We will store the last N inputs starting from start_tick (in case of packet loss)
pub struct InputMessage<T: UserAction> {
    pub(crate) end_tick: Tick,
    // first element is tick end_tick-N+1, last element is end_tick
    pub(crate) global_diffs: Vec<Vec<ActionDiff<T>>>,
    pub(crate) per_entity_diffs: Vec<(Entity, Vec<Vec<ActionDiff<T>>>)>,
}

impl<'a, A: UserAction> MapEntities<'a> for InputMessage<A> {
    fn map_entities(&mut self, entity_mapper: Box<dyn EntityMapper + 'a>) {
        self.per_entity_diffs.iter_mut().for_each(|(entity, _)| {
            if let Some(new_entity) = entity_mapper.map(*entity) {
                *entity = new_entity;
            }
        });
    }

    fn entities(&self) -> EntityHashSet<Entity> {
        self.per_entity_diffs
            .iter()
            .map(|(entity, _)| *entity)
            .collect()
    }
}

impl<T: UserAction> InputMessage<T> {
    pub fn new(end_tick: Tick) -> Self {
        Self {
            end_tick,
            global_diffs: vec![],
            per_entity_diffs: Default::default(),
        }
    }
}

impl<T: UserAction> InputBuffer<T> {
    // Note: we expect this to be set every tick?
    //  i.e. there should be an ActionState for every tick, even if the action is None
    pub(crate) fn set(&mut self, tick: Tick, value: ActionState<T>) {
        let Some(start_tick) = self.start_tick else {
            // initialize the buffer
            self.start_tick = Some(tick);
            self.buffer.push_back(BufferItem::Data(value));
            return;
        };

        // cannot set lower values than start_tick
        if tick < start_tick {
            return;
        }

        let end_tick = start_tick + (self.buffer.len() as i16 - 1);
        if tick > end_tick {
            // TODO: think about whether this is correct or not, it is correct if we always call set()
            //  with monotonically increasing ticks, which I think is the case
            //  maybe that's not correct because the timing information should be different? (i.e. I should tick the action-states myself
            //  and set them)
            // fill the ticks between end_tick and tick with a copy of the current ActionState
            for _ in 0..(tick - end_tick - 1) {
                self.buffer.push_back(BufferItem::SameAsPrecedent);
            }
            // add a new value to the buffer, which we will override below
            self.buffer.push_back(BufferItem::Absent);
        }

        // check if the value is the same as the precedent tick, in which case we compress it
        let mut same_as_precedent = false;
        if let Some(action_state) = self.get(tick - 1) {
            if action_state == &value {
                same_as_precedent = true;
            }
        }

        // safety: we are guaranteed that the tick is in the buffer
        let entry = self.buffer.get_mut((tick - start_tick) as usize).unwrap();

        if same_as_precedent {
            *entry = BufferItem::SameAsPrecedent;
        } else {
            *entry = BufferItem::Data(value);
        }
    }

    /// Remove all the inputs that are older than the given tick, then return the input
    /// for the given tick
    pub(crate) fn pop(&mut self, tick: Tick) -> Option<ActionState<T>> {
        let Some(start_tick) = self.start_tick else {
            return None;
        };
        if tick < start_tick {
            return None;
        }
        if tick > start_tick + (self.buffer.len() as i16 - 1) {
            // pop everything
            self.buffer = VecDeque::new();
            self.start_tick = Some(tick + 1);
            return None;
        }
        // info!(
        //     "buffer: {:?}. start_tick: {:?}, tick: {:?}",
        //     self.buffer, self.start_tick, tick
        // );

        // popped will represent the last value popped
        let mut popped = BufferItem::Absent;
        for _ in 0..(tick + 1 - start_tick) {
            // front is the oldest value
            let data = self.buffer.pop_front();
            if let Some(BufferItem::Data(value)) = data {
                popped = BufferItem::Data(value);
            }
        }
        self.start_tick = Some(tick + 1);

        // if the next value after we popped was 'SameAsPrecedent', we need to override it with an actual value
        if let Some(BufferItem::SameAsPrecedent) = self.buffer.front() {
            *self.buffer.front_mut().unwrap() = popped.clone();
        }

        if let BufferItem::Data(value) = popped {
            return Some(value);
        } else {
            return None;
        }
    }

    /// Get the ActionState for the given tick
    pub(crate) fn get(&self, tick: Tick) -> Option<&ActionState<T>> {
        let Some(start_tick) = self.start_tick else {
            return None;
        };
        if tick < start_tick || tick > start_tick + (self.buffer.len() as i16 - 1) {
            return None;
        }
        let data = self.buffer.get((tick.0 - start_tick.0) as usize).unwrap();
        match data {
            BufferItem::Absent => None,
            BufferItem::SameAsPrecedent => {
                // get the data from the preceding tick
                self.get(tick - 1)
            }
            BufferItem::Data(data) => Some(data),
        }
    }

    // Convert the last N ticks up to end_tick included into a compressed message that we can send to the server
    // Return None if the last N inputs are all Absent
    pub(crate) fn add_to_message(
        &self,
        message: &mut InputMessage<T>,
        end_tick: Tick,
        num_ticks: u16,
        entity: Option<Entity>,
    ) {
        let mut inputs = Vec::new();
        // start with the first value
        let start_tick = Tick(end_tick.0) - num_ticks + 1;
        let get_diffs = |action_state: &ActionState<T>| {
            action_state
                .get_just_pressed()
                .into_iter()
                .map(|a| ActionDiff::Pressed(a))
                .chain(
                    action_state
                        .get_just_released()
                        .into_iter()
                        .map(|a| ActionDiff::Released(a)),
                )
                .collect::<Vec<ActionDiff<T>>>()
        };
        for delta in 0..num_ticks {
            let tick = start_tick + Tick(delta);
            let diffs = self.get(tick).map_or(vec![], get_diffs);
            inputs.push(diffs);
        }
        match entity {
            None => message.global_diffs = inputs,
            Some(e) => {
                message.per_entity_diffs.push((e, inputs));
            }
        }
    }
}

/// The `ActionDiffBuffer` stores the ActionDiff received from the client for each tick
#[derive(Resource, Component, Debug)]
pub(crate) struct ActionDiffBuffer<A: UserAction> {
    start_tick: Option<Tick>,
    buffer: VecDeque<Vec<ActionDiff<A>>>,
}

impl<A: UserAction> Default for ActionDiffBuffer<A> {
    fn default() -> Self {
        Self {
            start_tick: None,
            buffer: VecDeque::new(),
        }
    }
}

impl<A: UserAction> ActionDiffBuffer<A> {
    pub(crate) fn set(&mut self, tick: Tick, diffs: Vec<ActionDiff<A>>) {
        let Some(start_tick) = self.start_tick else {
            // initialize the buffer
            self.start_tick = Some(tick);
            self.buffer.push_back(diffs);
            return;
        };

        // cannot set lower values than start_tick
        if tick < start_tick {
            return;
        }

        let end_tick = start_tick + (self.buffer.len() as i16 - 1);
        if tick > end_tick {
            // fill the ticks between end_tick and tick with a copy of the current ActionState
            for _ in 0..(tick - end_tick - 1) {
                self.buffer.push_back(vec![]);
            }
            // add a new value to the buffer, which we will override below
            self.buffer.push_back(diffs);
            return;
        }
        // safety: we are guaranteed that the tick is in the buffer
        let entry = self.buffer.get_mut((tick - start_tick) as usize).unwrap();
        *entry = diffs;
    }

    /// Remove all the diffs that are older than the given tick, then return the diffs
    /// for the given tick
    pub(crate) fn pop(&mut self, tick: Tick) -> Vec<ActionDiff<A>> {
        let Some(start_tick) = self.start_tick else {
            return vec![];
        };
        if tick < start_tick {
            return vec![];
        }
        if tick > start_tick + (self.buffer.len() as i16 - 1) {
            // pop everything
            self.buffer = VecDeque::new();
            self.start_tick = Some(tick + 1);
            return vec![];
        }

        for _ in 0..(tick - start_tick) {
            // front is the oldest value
            self.buffer.pop_front();
        }
        self.start_tick = Some(tick + 1);

        self.buffer.pop_front().unwrap_or(vec![])
    }

    /// Get the ActionState for the given tick
    pub(crate) fn get(&self, tick: Tick) -> Vec<ActionDiff<A>> {
        let Some(start_tick) = self.start_tick else {
            return vec![];
        };
        self.buffer
            .get((tick.0 - start_tick.0) as usize)
            .unwrap_or(&vec![])
            .clone()
    }
    pub(crate) fn update_from_message(&mut self, end_tick: Tick, diffs: Vec<Vec<ActionDiff<A>>>) {
        let message_start_tick = end_tick - diffs.len() as u16 + 1;
        if self.start_tick.is_none() {
            // initialize the buffer
            self.start_tick = Some(message_start_tick);
        };
        let start_tick = self.start_tick.unwrap();

        for (delta, diffs_for_tick) in diffs.into_iter().enumerate() {
            let tick = message_start_tick + Tick(delta as u16);
            self.set(tick, diffs_for_tick);
        }
    }
}

// TODO: update from message

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::prelude::Reflect;

    #[derive(
        Serialize, Deserialize, Copy, Clone, Eq, PartialEq, Debug, Hash, Reflect, Actionlike,
    )]
    enum Action {
        Jump,
    }

    impl UserAction for Action {}

    #[test]
    fn test_get_set_pop() {
        let mut input_buffer = InputBuffer::default();

        let mut a1 = ActionState::default();
        a1.press(Action::Jump);
        a1.action_data_mut(Action::Jump).value = 0.0;
        let mut a2 = ActionState::default();
        a2.press(Action::Jump);
        a1.action_data_mut(Action::Jump).value = 1.0;
        input_buffer.set(Tick(3), a1.clone());
        input_buffer.set(Tick(6), a2.clone());
        input_buffer.set(Tick(7), a2.clone());

        assert_eq!(input_buffer.start_tick, Some(Tick(3)));
        assert_eq!(input_buffer.buffer.len(), 5);

        assert_eq!(input_buffer.get(Tick(3)), Some(&a1));
        assert_eq!(input_buffer.get(Tick(4)), Some(&a1));
        assert_eq!(input_buffer.get(Tick(5)), Some(&a1));
        assert_eq!(input_buffer.get(Tick(6)), Some(&a2));
        assert_eq!(input_buffer.get(Tick(8)), None);

        assert_eq!(input_buffer.pop(Tick(4)), Some(a1.clone()));
        assert_eq!(input_buffer.start_tick, Some(Tick(5)));
        assert_eq!(input_buffer.buffer.len(), 3);

        // the oldest element has been updated from `SameAsPrecedent` to `Data`
        assert_eq!(
            input_buffer.buffer.front().unwrap(),
            &BufferItem::Data(a1.clone())
        );
        assert_eq!(input_buffer.pop(Tick(7)), Some(a2.clone()));
        assert_eq!(input_buffer.start_tick, Some(Tick(8)));
        assert_eq!(input_buffer.buffer.len(), 0);
    }

    #[test]
    fn test_create_message() {
        let mut input_buffer = InputBuffer::default();

        let mut a1 = ActionState::default();
        a1.press(Action::Jump);
        a1.action_data_mut(Action::Jump).value = 0.0;
        let mut a2 = ActionState::default();
        a2.press(Action::Jump);
        a1.action_data_mut(Action::Jump).value = 1.0;
        input_buffer.set(Tick(3), a1.clone());
        input_buffer.set(Tick(4), ActionState::default());
        input_buffer.set(Tick(5), ActionState::default());
        input_buffer.set(Tick(6), ActionState::default());
        input_buffer.set(Tick(7), a2.clone());

        let end_tick = Tick(10);
        let mut message = InputMessage::<Action>::new(end_tick);

        input_buffer.add_to_message(&mut message, end_tick, 9, None);
        assert_eq!(
            message,
            InputMessage {
                end_tick: Tick(10),
                global_diffs: vec![
                    vec![], // tick 2
                    vec![ActionDiff::Pressed(Action::Jump)],
                    vec![],
                    vec![],
                    vec![],
                    vec![ActionDiff::Pressed(Action::Jump)],
                    vec![],
                    vec![],
                    vec![],
                ],
                per_entity_diffs: vec![],
            }
        );
    }

    #[test]
    fn test_update_from_message() {
        let mut diff_buffer = ActionDiffBuffer::default();

        let end_tick = Tick(20);
        let diffs = vec![
            vec![],
            vec![ActionDiff::Pressed(Action::Jump)],
            vec![],
            vec![],
            vec![],
            vec![ActionDiff::Pressed(Action::Jump)],
            vec![],
            vec![],
            vec![],
        ];

        diff_buffer.update_from_message(end_tick, diffs);

        assert_eq!(diff_buffer.get(Tick(20)), vec![]);
        assert_eq!(diff_buffer.get(Tick(19)), vec![]);
        assert_eq!(diff_buffer.get(Tick(18)), vec![]);
        assert_eq!(
            diff_buffer.get(Tick(17)),
            vec![ActionDiff::Pressed(Action::Jump)]
        );
        assert_eq!(diff_buffer.get(Tick(16)), vec![]);
        assert_eq!(diff_buffer.get(Tick(15)), vec![]);
        assert_eq!(diff_buffer.get(Tick(14)), vec![]);
        assert_eq!(
            diff_buffer.get(Tick(13)),
            vec![ActionDiff::Pressed(Action::Jump)]
        );
        assert_eq!(diff_buffer.get(Tick(12)), vec![]);
    }
}
