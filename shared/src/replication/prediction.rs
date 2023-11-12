use crate::client::prediction::{Rollback, RollbackState};
use crate::tick::Tick;
use bevy::prelude::{Component, Res, Resource};
use bitcode::__private::Serialize;
use serde::Deserialize;

// TODO: Right now we use the approach that we add an extra component to the Protocol of components to be replicated.
//  that's pretty dangerous because it's now hard for the user to derive new traits.
//  let's think of another approach later.
#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ShouldBePredicted;

/// Returns true if we are doing rollback
pub fn is_in_rollback(rollback: Res<Rollback>) -> bool {
    match rollback.state {
        RollbackState::ShouldRollback { .. } => true,
        _ => false,
    }
}
