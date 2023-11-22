use bevy::prelude::Component;
use serde::{Deserialize, Serialize};

// TODO: Right now we use the approach that we add an extra component to the Protocol of components to be replicated.
//  that's pretty dangerous because it's now hard for the user to derive new traits.
//  let's think of another approach later.
#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ShouldBeInterpolated;
