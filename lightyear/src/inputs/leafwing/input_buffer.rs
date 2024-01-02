use std::collections::VecDeque;
use std::fmt::Debug;

use bevy::prelude::{Component, Entity, Resource};
use bevy::utils::{EntityHashMap, EntityHashSet};
use leafwing_input_manager::common_conditions::action_just_pressed;
use leafwing_input_manager::prelude::ActionState;
use leafwing_input_manager::Actionlike;
use serde::{Deserialize, Serialize};

use crate::prelude::{EntityMapper, MapEntities};
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

// NOTE: right now, for simplicity, we will send all the action-diffs for all entities in one single message.
// TODO: improve this data structure
#[derive(Resource, Component, Debug)]
pub(crate) struct InputBuffer<A: UserAction> {
    start_tick: Tick,
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
            start_tick: Tick(0),
            buffer: VecDeque::new(),
        }
    }
}

/// Whether an action was just pressed or released. We can use this to reconstruct the ActionState
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub enum ActionDiff<T: UserAction> {
    Pressed(T),
    Released(T),
}

// TODO: use Mode to specify how to serialize a message (serde vs bitcode)! + can specify custom serialize function as well (similar to interpolation mode)
#[derive(MessageInternal, Serialize, Deserialize, Clone, PartialEq, Debug)]
#[message(custom_map)]
/// We serialize the inputs by sending only the ActionDiffs of the last few ticks
/// We will store the last N inputs starting from start_tick (in case of packet loss)
pub struct InputMessage<T: UserAction> {
    end_tick: Tick,
    // first element is tick end_tick-N+1, last element is end_tick
    global_diffs: Vec<Vec<ActionDiff<T>>>,
    per_entity_diffs: EntityHashMap<Entity, Vec<Vec<ActionDiff<T>>>>,
}

impl<'a, A: UserAction> MapEntities<'a> for InputMessage<A> {
    fn map_entities(&mut self, entity_mapper: Box<dyn EntityMapper + 'a>) {
        self.per_entity_diffs.drain().for_each(|(entity, diffs)| {
            if let Some(new_entity) = entity_mapper.map(entity) {
                self.per_entity_diffs.insert(new_entity, diffs);
            }
        });
    }

    fn entities(&self) -> EntityHashSet<Entity> {
        self.per_entity_diffs.keys().copied().collect()
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
        // cannot set lower values than start_tick
        if tick < self.start_tick {
            return;
        }

        let end_tick = self.start_tick + (self.buffer.len() as i16 - 1);
        if tick > end_tick {
            // TODO: think about whether this is correct or not, it is correct if we always call set()
            //  with monotonically increasing ticks, which I think is the case
            // fill the ticks between end_tick and tick with a copy of the current ActionState
            for _ in 0..(tick - end_tick - 1) {
                self.buffer.push_back(BufferItem::SameAsPrecedent);
            }
            // add a new value to the buffer, which we will override below
            self.buffer.push_back(BufferItem::Absent);
        }

        // safety: we are guaranteed that the tick is in the buffer
        let entry = self
            .buffer
            .get_mut((tick - self.start_tick) as usize)
            .unwrap();

        // check what the previous value was
        if let Some(action_state) = self.get(tick - (1 + self.start_tick)) {
            if action_state == &value {
                *entry = BufferItem::SameAsPrecedent;
            } else {
                *entry = BufferItem::Data(value);
            }
        } else {
            *entry = BufferItem::Data(value);
        }
    }

    /// Remove all the inputs that are older than the given tick, then return the input
    /// for the given tick
    pub(crate) fn pop(&mut self, tick: Tick) -> Option<ActionState<T>> {
        if tick < self.start_tick {
            return None;
        }
        if tick > self.start_tick + (self.buffer.len() as i16 - 1) {
            // pop everything
            self.buffer = VecDeque::new();
            self.start_tick = tick + 1;
            return None;
        }
        // info!(
        //     "buffer: {:?}. start_tick: {:?}, tick: {:?}",
        //     self.buffer, self.start_tick, tick
        // );

        // popped will represent the last value popped
        let mut popped = BufferItem::Absent;
        for _ in 0..(tick + 1 - self.start_tick) {
            // front is the oldest value
            let data = self.buffer.pop_front();
            if let Some(BufferItem::Data(value)) = data {
                popped = BufferItem::Data(value);
            }
        }
        self.start_tick = tick + 1;

        // if the next value after we popped was 'SameAsPrecedent', we need to override it with an actual value
        if let Some(BufferItem::SameAsPrecedent) = self.buffer.front() {
            *self.buffer.front_mut().unwrap() = popped.clone();
        }

        if let Some(BufferItem::Data(value)) = popped {
            return Some(value);
        } else {
            return None;
        }
    }

    pub(crate) fn get(&self, tick: Tick) -> Option<&ActionState<T>> {
        if tick < self.start_tick || tick > self.start_tick + (self.buffer.len() as i16 - 1) {
            return None;
        }
        let data = self
            .buffer
            .get((tick.0 - self.start_tick.0) as usize)
            .unwrap();
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
            let diffs = self.get(start_tick).map_or(vec![], get_diffs);
            inputs.push(diffs);
        }
        match entity {
            None => message.global_diffs = inputs,
            Some(e) => {
                message.per_entity_diffs.insert(e, inputs);
            }
        }
    }
}

/// The `ActionDiffBuffer` stores the ActionDiff received from the client for each tick
#[derive(Resource, Component, Debug)]
pub(crate) struct ActionDiffBuffer<A: UserAction> {
    start_tick: Tick,
    buffer: VecDeque<Vec<ActionDiff<A>>>,
}

impl<A: UserAction> Default for ActionDiffBuffer<A> {
    fn default() -> Self {
        Self {
            start_tick: Tick(0),
            buffer: VecDeque::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::prelude::Reflect;

    #[derive(
        Serialize, Deserialize, Copy, Clone, Eq, PartialEq, Debug, Hash, Reflect, Actionlike,
    )]
    enum Action {
        Jump(usize),
    }

    impl UserAction for Action {}

    #[test]
    fn test_get_set_pop() {
        let mut input_buffer = InputBuffer::default();

        let mut a1 = ActionState::default();
        a1.press(Action::Jump(0));
        let mut a2 = ActionState::default();
        a2.press(Action::Jump(1));
        input_buffer.set(Tick(4), a1);
        input_buffer.set(Tick(6), a2);
        input_buffer.set(Tick(7), a2.clone());

        assert_eq!(input_buffer.start_tick, Tick(0));
        //
        // assert_eq!(input_buffer.get(Tick(4)), Some(&0));
        // assert_eq!(input_buffer.get(Tick(5)), None);
        // assert_eq!(input_buffer.get(Tick(6)), Some(&1));
        // assert_eq!(input_buffer.get(Tick(8)), None);
        //
        // assert_eq!(input_buffer.pop(Tick(5)), None);
        // assert_eq!(input_buffer.start_tick, Tick(6));
        // assert_eq!(input_buffer.pop(Tick(7)), Some(1));
        // assert_eq!(input_buffer.start_tick, Tick(8));
        // assert_eq!(input_buffer.buffer.len(), 0);
    }

    // #[test]
    // fn test_create_message() {
    //     let mut input_buffer = InputBuffer::default();
    //
    //     input_buffer.set(Tick(4), Some(0));
    //     input_buffer.set(Tick(6), Some(1));
    //     input_buffer.set(Tick(7), Some(1));
    //
    //     let message = input_buffer.create_message(Tick(10), 8);
    //     assert_eq!(
    //         message,
    //         InputMessage {
    //             end_tick: Tick(10),
    //             inputs: vec![
    //                 InputData::Absent,
    //                 InputData::Input(0),
    //                 InputData::Absent,
    //                 InputData::Input(1),
    //                 InputData::SameAsPrecedent,
    //                 InputData::Absent,
    //                 InputData::SameAsPrecedent,
    //                 InputData::SameAsPrecedent,
    //             ],
    //         }
    //     );
    // }

    // #[test]
    // fn test_update_from_message() {
    //     let mut input_buffer = InputBuffer::default();
    //
    //     let message = InputMessage {
    //         end_tick: Tick(20),
    //         inputs: vec![
    //             InputData::Absent,
    //             InputData::Input(0),
    //             InputData::Absent,
    //             InputData::Input(1),
    //             InputData::SameAsPrecedent,
    //             InputData::Absent,
    //             InputData::SameAsPrecedent,
    //             InputData::SameAsPrecedent,
    //         ],
    //     };
    //     input_buffer.update_from_message(message);
    //
    //     assert_eq!(input_buffer.get(Tick(20)), None);
    //     assert_eq!(input_buffer.get(Tick(19)), None);
    //     assert_eq!(input_buffer.get(Tick(18)), None);
    //     assert_eq!(input_buffer.get(Tick(17)), Some(&1));
    //     assert_eq!(input_buffer.get(Tick(16)), Some(&1));
    //     assert_eq!(input_buffer.get(Tick(15)), None);
    //     assert_eq!(input_buffer.get(Tick(14)), Some(&0));
    //     assert_eq!(input_buffer.get(Tick(13)), None);
    // }
}
