//! Handles client-side prediction
use std::fmt::Debug;

use bevy::prelude::*;
use tracing::error;

pub use despawn::PredictionDespawnCommandsExt;
pub use plugin::add_prediction_systems;
pub use predicted_history::{ComponentState, PredictionHistory};

use crate::client::components::Confirmed;
use crate::client::connection::ConnectionManager;
use crate::client::events::ComponentInsertEvent;
use crate::client::prediction::resource::PredictionManager;

use crate::shared::replication::components::{PrePredicted, Replicate, ShouldBePredicted};
use crate::shared::tick_manager::Tick;

pub(crate) mod correction;
mod despawn;
pub mod plugin;
mod pre_prediction;
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
