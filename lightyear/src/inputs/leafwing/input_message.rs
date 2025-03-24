use crate::client::interpolation::plugin::InterpolationDelay;
use crate::inputs::leafwing::action_diff::ActionDiff;
use crate::inputs::leafwing::input_buffer::InputBuffer;
use crate::prelude::{Deserialize, LeafwingUserAction, Serialize, Tick};
use bevy::ecs::entity::MapEntities;
use bevy::prelude::{Entity, EntityMapper, Reflect};
use core::fmt::{Formatter, Write};
use leafwing_input_manager::action_state::ActionState;
use leafwing_input_manager::Actionlike;

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Reflect)]
pub(crate) struct PerTargetData<A: Actionlike> {
    pub(crate) target: InputTarget,
    // The ActionState is the state at tick end_tick-N
    pub(crate) start_state: ActionState<A>,
    // ActionDiffs to apply to the ActionState for ticks `end_tick-N+1` to `end_tick` (included)
    pub(crate) diffs: Vec<Vec<ActionDiff<A>>>,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Reflect)]
/// We serialize the inputs by sending, for each entity:
/// - the ActionState at a given start tick
/// - then the ActionDiffs for all the ticks after that, up to end_tick
///
/// (We do this to make sure that we can reconstruct the ActionState at any tick,
/// even if we miss some inputs. This wouldn't be the case if we only send ActionDiffs)
pub struct InputMessage<A: Actionlike> {
    // TODO: avoid sending one extra byte for the option if no lag compensation! Maybe have a separate message type?
    //  or the message being lag-compensation-compatible is handled on the registry?
    /// Interpolation delay of the client at the time the message is sent
    ///
    /// We don't need any extra redundancy for the InterpolationDelay so we'll just send the value at `end_tick`.
    pub(crate) interpolation_delay: Option<InterpolationDelay>,
    pub(crate) end_tick: Tick,
    pub(crate) diffs: Vec<PerTargetData<A>>,
}

impl<A: LeafwingUserAction> core::fmt::Display for InputMessage<A> {
    fn fmt(&self, f: &mut Formatter<'_>) -> core::fmt::Result {
        let ty = A::short_type_path();

        if self.diffs.is_empty() {
            return write!(f, "EmptyInputMessage");
        }
        let start_tick = self.end_tick - Tick(self.diffs[0].diffs.len() as u16);
        let buffer_str = self
            .diffs
            .iter()
            .map(|data| {
                let mut str = format!("Entity: {:?}\n", data.target);
                let _ = writeln!(
                    &mut str,
                    "Tick: {start_tick:?}. StartState: {:?}",
                    data.start_state.get_pressed()
                );
                for (i, diffs) in data.diffs.iter().enumerate() {
                    let tick = start_tick + (i + 1) as i16;
                    let _ = writeln!(&mut str, "Tick: {}, Diffs: {:?}", tick, diffs);
                }
                str
            })
            .collect::<Vec<String>>()
            .join("\n");
        write!(f, "InputMessage<{:?}>:\n {}", ty, buffer_str)
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug, Reflect)]
pub enum InputTarget {
    /// the input is for a predicted or confirmed entity: on the client, the server's local entity is mapped to the client's confirmed entity
    Entity(Entity),
    /// the input is for a pre-predicted entity: on the server, the server's local entity is mapped to the client's pre-predicted entity
    PrePredictedEntity(Entity),
}

impl<A: LeafwingUserAction> MapEntities for InputMessage<A> {
    // NOTE: we do NOT map the entities for input-message because when already convert
    //  the entities on the message to the corresponding client entities when we write them
    //  in the input message

    // NOTE: we only map the inputs for the pre-predicted entities
    fn map_entities<M: EntityMapper>(&mut self, entity_mapper: &mut M) {
        self.diffs.iter_mut().for_each(|data| {
            if let InputTarget::PrePredictedEntity(ref mut e) = data.target {
                *e = entity_mapper.get_mapped(*e);
            }
        });
    }
}

impl<A: LeafwingUserAction> InputMessage<A> {
    pub fn new(end_tick: Tick) -> Self {
        Self {
            interpolation_delay: None,
            end_tick,
            diffs: vec![],
        }
    }

    /// Add the inputs for the `num_ticks` ticks starting from `self.end_tick - num_ticks + 1` up to `self.end_tick`
    ///
    /// If we don't have a starting `ActionState` from the `input_buffer`, we start from the first tick for which
    /// we have an `ActionState`.
    pub fn add_inputs(
        &mut self,
        num_ticks: u16,
        target: InputTarget,
        input_buffer: &InputBuffer<A>,
    ) {
        let mut diffs = Vec::new();
        // find the first tick for which we have an `ActionState` buffered
        let mut start_tick = self.end_tick - num_ticks + 1;
        while start_tick <= self.end_tick {
            if input_buffer.get(start_tick).is_some() {
                break;
            }
            start_tick += 1;
        }

        // there are no ticks for which we have an `ActionState` buffered, so we send nothing
        if start_tick > self.end_tick {
            return;
        }

        let start_state = input_buffer.get(start_tick).unwrap().clone();
        let mut tick = start_tick + 1;
        while tick <= self.end_tick {
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
        self.diffs.push(PerTargetData {
            target,
            start_state,
            diffs,
        });
    }

    // TODO: do we want to send the inputs if there are no diffs?
    pub fn is_empty(&self) -> bool {
        self.diffs.iter().all(|data| {
            data.diffs
                .iter()
                .all(|diffs_per_tick| diffs_per_tick.is_empty())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use leafwing_input_manager::Actionlike;

    #[derive(
        Serialize, Deserialize, Copy, Clone, Eq, PartialEq, Debug, Hash, Reflect, Actionlike,
    )]
    enum Action {
        Jump,
    }

    #[test]
    fn test_generate_input_message_no_start_input() {
        let input_buffer = InputBuffer::default();
        let mut input_message = InputMessage::<Action>::new(Tick(10));
        input_message.add_inputs(5, InputTarget::Entity(Entity::PLACEHOLDER), &input_buffer);
        assert_eq!(
            input_message,
            InputMessage {
                interpolation_delay: None,
                end_tick: Tick(10),
                diffs: vec![],
            }
        );
        assert!(input_message.is_empty());
    }

    // #[test]
    // fn test_create_message() {
    //     let mut input_buffer = InputBuffer::default();
    //     let mut action_state = ActionState::default();
    //     input_buffer.set(Tick(2), &ActionState::default());
    //     action_state.press(&Action::Jump);
    //     input_buffer.set(Tick(3), &action_state);
    //     action_state.release(&Action::Jump);
    //
    //     diff_buffer.set(
    //         Tick(3),
    //         &vec![ActionDiff::Pressed {
    //             action: Action::Jump,
    //         }],
    //     );
    //     diff_buffer.set(
    //         Tick(7),
    //         &vec![ActionDiff::Released {
    //             action: Action::Jump,
    //         }],
    //     );
    //
    //     let entity = Entity::from_raw(0);
    //     let end_tick = Tick(10);
    //     let mut message = InputMessage::<Action>::new(end_tick);
    //
    //     message.add_inputs(9, InputTarget::Entity(entity), &diff_buffer, &input_buffer);
    //     assert_eq!(
    //         message,
    //         InputMessage {
    //             end_tick: Tick(10),
    //             diffs: vec![(
    //                 InputTarget::Entity(entity),
    //                 ActionState::default(),
    //                 vec![
    //                     vec![ActionDiff::Pressed {
    //                         action: Action::Jump
    //                     }],
    //                     vec![],
    //                     vec![],
    //                     vec![],
    //                     vec![ActionDiff::Released {
    //                         action: Action::Jump
    //                     }],
    //                     vec![],
    //                     vec![],
    //                     vec![],
    //                 ]
    //             )],
    //         }
    //     );
    // }
}
