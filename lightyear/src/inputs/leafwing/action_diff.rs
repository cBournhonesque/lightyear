use crate::prelude::{Deserialize, LeafwingUserAction, Serialize};
use bevy::math::Vec2;
use bevy::prelude::Reflect;
use leafwing_input_manager::action_state::ActionState;
use leafwing_input_manager::axislike::DualAxisData;

/// Stores presses and releases of buttons without timing information
///
/// Used to serialize the difference between two `ActionState` in order to send less data
/// over the network when sending inputs.
///
/// An `ActionState` can be fully reconstructed from a stream of `ActionDiff`
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Reflect)]
pub enum ActionDiff<A> {
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
    ValueChanged {
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
}

impl<A: LeafwingUserAction> ActionDiff<A> {
    /// Creates a list of `ActionDiff` from the difference between two `ActionState`
    /// Used to have a smaller serialized size when sending inputs over the network
    pub(crate) fn create(before: &ActionState<A>, after: &ActionState<A>) -> Vec<Self> {
        let mut diffs = vec![];
        for action in after.keys().iter() {
            let action_data_after = after.action_data(action);
            if let Some(action_data_after) = action_data_after {
                let action_data_before = before.action_data(action);
                // TODO: handle 'consume'? handle 'timing'?
                if let Some(action_data_before) = action_data_before {
                    if let Some(axis_pair_after) = action_data_after.axis_pair {
                        if let Some(axis_pair_before) = action_data_before.axis_pair {
                            if axis_pair_after != axis_pair_before {
                                diffs.push(ActionDiff::AxisPairChanged {
                                    action: action.clone(),
                                    axis_pair: axis_pair_after.into(),
                                });
                            }
                        } else {
                            diffs.push(ActionDiff::AxisPairChanged {
                                action: action.clone(),
                                axis_pair: axis_pair_after.into(),
                            });
                        }
                    } else if action_data_after.value != 1.0
                        && action_data_after.value != 0.0
                        && action_data_before.value != action_data_after.value
                    {
                        diffs.push(ActionDiff::ValueChanged {
                            action: action.clone(),
                            value: action_data_after.value,
                        });
                    } else if action_data_after.state.pressed()
                        && !action_data_before.state.pressed()
                    {
                        diffs.push(ActionDiff::Pressed {
                            action: action.clone(),
                        });
                    } else if !action_data_after.state.pressed()
                        && action_data_before.state.pressed()
                    {
                        diffs.push(ActionDiff::Released {
                            action: action.clone(),
                        });
                    }
                } else {
                    if let Some(axis_pair_after) = action_data_after.axis_pair {
                        diffs.push(ActionDiff::AxisPairChanged {
                            action: action.clone(),
                            axis_pair: axis_pair_after.into(),
                        });
                    } else if action_data_after.value != 1.0 {
                        diffs.push(ActionDiff::ValueChanged {
                            action: action.clone(),
                            value: action_data_after.value,
                        });
                    } else if action_data_after.state.pressed() {
                        diffs.push(ActionDiff::Pressed {
                            action: action.clone(),
                        });
                    } else if !action_data_after.state.pressed() {
                        diffs.push(ActionDiff::Released {
                            action: action.clone(),
                        });
                    }
                }
            } else {
                unreachable!("ActionData_after should have been initialized");
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
                // Pressing will initialize the ActionData if it doesn't exist
                action_state.action_data_mut(&action).unwrap().value = 1.0;
            }
            ActionDiff::Released { action } => {
                action_state.release(&action);
                // Releasing will initialize the ActionData if it doesn't exist
                let action_data = action_state.action_data_mut(&action).unwrap();
                action_data.value = 0.;
                action_data.axis_pair = None;
            }
            ActionDiff::ValueChanged { action, value } => {
                action_state.press(&action);
                // Pressing will initialize the ActionData if it doesn't exist
                action_state.action_data_mut(&action).unwrap().value = value;
            }
            ActionDiff::AxisPairChanged { action, axis_pair } => {
                action_state.press(&action);
                // Pressing will initialize the ActionData if it doesn't exist
                let action_data = action_state.action_data_mut(&action).unwrap();
                action_data.axis_pair = Some(DualAxisData::from_xy(axis_pair));
                action_data.value = axis_pair.length();
            }
        };
    }
}

#[cfg(test)]
mod tests {
    use crate::prelude::{Deserialize, Serialize};
    use bevy::prelude::Reflect;
    use leafwing_input_manager::Actionlike;

    #[derive(
        Serialize, Deserialize, Copy, Clone, Eq, PartialEq, Debug, Hash, Reflect, Actionlike,
    )]
    enum Action {
        Jump,
    }

    // fn test_diff() {
    //     let mut action_state = ActionState::new();
    //     action_state.press(&Action::Jump);
    //     action_state.action_data_mut(&Action::Jump).unwrap().value = 0.5;
    //     let mut action_state2 = action_state.clone();
    //     action_state2.action_data_mut(&Action::Jump).unwrap().value = 0.75;
    //     let diff = ActionDiff::create(&action_state, &action_state2);
    //     assert_eq!(diff.len(), 1);
    //     let mut action_state3 = action_state.clone();
    //     diff[0].apply(&mut action_state3);
    //     assert_eq!(action_state2, action_state3);
    // }
}
