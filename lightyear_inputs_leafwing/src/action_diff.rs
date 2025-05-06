use crate::action_state::LeafwingUserAction;
#[cfg(not(feature = "std"))]
use alloc::{format, string::String, vec, vec::Vec};
use bevy::math::{Vec2, Vec3};
use bevy::prelude::Reflect;
use leafwing_input_manager::action_state::{ActionKindData, ActionState};
use serde::{Deserialize, Serialize};

// TODO: can reuse the ActionDiff from leafwing_input_manager?
/// Stores presses and releases of buttons without timing information
///
/// Used to serialize the difference between two `ActionState` in order to send less data
/// over the network when sending inputs.
///
/// An `ActionState` can be fully reconstructed from a stream of `ActionDiff`
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Reflect)]
pub(crate) enum ActionDiff<A> {
    /// The action was pressed
    Pressed {
        /// The value of the action
        action: A,
    },
    /// The action was released
    Released {
        /// The value of the action
        action: A,
    },
    /// The value of the action changed
    AxisChanged {
        /// The value of the action
        action: A,
        /// The new value of the action
        value: f32,
    },
    /// The axis pair of the action changed
    AxisPairChanged {
        /// The value of the action
        action: A,
        /// The new value of the axis
        axis_pair: Vec2,
    },
    /// The axis triple of the action changed
    AxisTripleChanged {
        /// The value of the action
        action: A,
        /// The new value of the axis
        axis_triple: Vec3,
    },
}

impl<A: LeafwingUserAction> ActionDiff<A> {
    /// Creates a list of `ActionDiff` from the difference between two `ActionState`
    /// Used to have a smaller serialized size when sending inputs over the network
    pub(crate) fn create(before: &ActionState<A>, after: &ActionState<A>) -> Vec<Self> {
        let mut diffs = vec![];
        for (action, action_data_after) in after.all_action_data() {
            // no need to network disabled actions. Or should we network the default value?
            if action_data_after.disabled {
                continue;
            }
            if let Some(action_data_before) = before.action_data(action) {
                // TODO: handle disabled?
                match &action_data_after.kind_data {
                    ActionKindData::Button(button_data_after) => {
                        let button_data_before = match &action_data_before.kind_data {
                            ActionKindData::Button(button_data_before) => button_data_before,
                            _ => unreachable!(),
                        };
                        if button_data_after.state.pressed() && !button_data_before.state.pressed()
                        {
                            diffs.push(ActionDiff::Pressed { action: *action });
                        } else if !button_data_after.state.pressed()
                            && button_data_before.state.pressed()
                        {
                            diffs.push(ActionDiff::Released { action: *action });
                        }
                    }
                    ActionKindData::Axis(axis_data_after) => {
                        let axis_data_before = match &action_data_before.kind_data {
                            ActionKindData::Axis(axis_data_before) => axis_data_before,
                            _ => unreachable!(),
                        };
                        if axis_data_after.value != axis_data_before.value {
                            diffs.push(ActionDiff::AxisChanged {
                                action: *action,
                                value: axis_data_after.value,
                            });
                        }
                    }
                    ActionKindData::DualAxis(dual_axis_after) => {
                        let dual_axis_before = match &action_data_before.kind_data {
                            ActionKindData::DualAxis(dual_axis_before) => dual_axis_before,
                            _ => unreachable!(),
                        };
                        if dual_axis_after.pair != dual_axis_before.pair {
                            diffs.push(ActionDiff::AxisPairChanged {
                                action: *action,
                                axis_pair: dual_axis_after.pair,
                            });
                        }
                    }
                    ActionKindData::TripleAxis(triple_axis_after) => {
                        let triple_axis_before = match &action_data_before.kind_data {
                            ActionKindData::TripleAxis(triple_axis_before) => triple_axis_before,
                            _ => unreachable!(),
                        };
                        if triple_axis_after.triple != triple_axis_before.triple {
                            diffs.push(ActionDiff::AxisTripleChanged {
                                action: *action,
                                axis_triple: triple_axis_after.triple,
                            });
                        }
                    }
                }
            } else {
                match &action_data_after.kind_data {
                    ActionKindData::Button(button) => {
                        if button.pressed() {
                            diffs.push(ActionDiff::Pressed { action: *action });
                        } else {
                            diffs.push(ActionDiff::Released { action: *action });
                        }
                    }
                    ActionKindData::Axis(axis) => {
                        diffs.push(ActionDiff::AxisChanged {
                            action: *action,
                            value: axis.value,
                        });
                    }
                    ActionKindData::DualAxis(dual_axis) => {
                        diffs.push(ActionDiff::AxisPairChanged {
                            action: *action,
                            axis_pair: dual_axis.pair,
                        });
                    }
                    ActionKindData::TripleAxis(triple_axis) => {
                        diffs.push(ActionDiff::AxisTripleChanged {
                            action: *action,
                            axis_triple: triple_axis.triple,
                        });
                    }
                }
            }
        }
        diffs
    }

    /// Applies an [`ActionDiff`] (usually received over the network) to the [`ActionState`].
    ///
    /// This lets you reconstruct an [`ActionState`] from a stream of [`ActionDiff`]s
    pub(crate) fn apply(self, action_state: &mut ActionState<A>) {
        match self {
            ActionDiff::Pressed { action } => {
                action_state.press(&action);
            }
            ActionDiff::Released { action } => {
                action_state.release(&action);
            }
            ActionDiff::AxisChanged { action, value } => {
                action_state.axis_data_mut_or_default(&action).value = value;
            }
            ActionDiff::AxisPairChanged { action, axis_pair } => {
                action_state.set_axis_pair(&action, axis_pair);
            }
            ActionDiff::AxisTripleChanged {
                action,
                axis_triple,
            } => {
                action_state.set_axis_triple(&action, axis_triple);
            }
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::{
        math::{Vec2, Vec3},
        prelude::Reflect,
    };
    use leafwing_input_manager::{action_state::ActionState, Actionlike};

    #[derive(
        Serialize, Deserialize, Copy, Clone, Eq, PartialEq, Debug, Hash, Reflect, Actionlike,
    )]
    enum Action {
        #[actionlike(Axis)]
        Axis,
        #[actionlike(DualAxis)]
        DualAxis,
        #[actionlike(TripleAxis)]
        TripleAxis,
    }

    #[test]
    fn test_axis_diff() {
        let mut action_state = ActionState::default();
        action_state.set_value(&Action::Axis, 0.5);
        let mut action_state2 = action_state.clone();
        action_state2.set_value(&Action::Axis, 0.75);
        let diff = ActionDiff::create(&action_state, &action_state2);
        assert_eq!(diff.len(), 1);
        let mut action_state3 = action_state.clone();
        diff[0].apply(&mut action_state3);
        assert_eq!(action_state2, action_state3);
    }

    #[test]
    fn test_dual_axis_diff() {
        let mut action_state = ActionState::default();
        action_state.set_axis_pair(&Action::DualAxis, Vec2::new(0.5, 0.75));
        let mut action_state2 = action_state.clone();
        action_state2.set_axis_pair(&Action::DualAxis, Vec2::new(0.75, 0.5));
        let diff = ActionDiff::create(&action_state, &action_state2);
        assert_eq!(diff.len(), 1);
        let mut action_state3 = action_state.clone();
        diff[0].apply(&mut action_state3);
        assert_eq!(action_state2, action_state3);
    }

    #[test]
    fn test_triple_axis_diff() {
        let mut action_state = ActionState::default();
        action_state.set_axis_triple(&Action::TripleAxis, Vec3::new(0.5, 0.75, 0.25));
        let mut action_state2 = action_state.clone();
        action_state2.set_axis_triple(&Action::TripleAxis, Vec3::new(0.75, 0.5, 0.25));
        let diff = ActionDiff::create(&action_state, &action_state2);
        assert_eq!(diff.len(), 1);
        let mut action_state3 = action_state.clone();
        diff[0].apply(&mut action_state3);
        assert_eq!(action_state2, action_state3);
    }
}
