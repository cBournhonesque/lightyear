//! Handles client-side prediction
use bevy::prelude::{Component, Entity, Reflect};
use std::fmt::Debug;

pub(crate) mod correction;
pub(crate) mod despawn;
pub mod diagnostics;
pub mod plugin;
pub(crate) mod pre_prediction;
pub mod predicted_history;
pub mod prespawn;
pub(crate) mod resource;
pub(crate) mod rollback;
pub mod spawn;

/// Marks an entity that is being predicted by the client
#[derive(Component, Debug, Reflect)]
pub struct Predicted {
    // This is an option because we could spawn pre-predicted entities on the client that exist before we receive
    // the corresponding confirmed entity
    pub confirmed_entity: Option<Entity>,
}
