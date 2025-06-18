use bevy::prelude::*;

/// Component added to client-side entities that are predicted.
///
/// Prediction allows the client to simulate the game state locally without waiting for server confirmation,
/// reducing perceived latency. This component links the predicted entity to its server-confirmed counterpart.
///
/// When an entity is marked as `Predicted`, the `PredictionPlugin` will:
/// - Store its component history.
/// - Rollback and re-simulate the entity when a server correction is received.
/// - Manage the relationship between the predicted entity and its corresponding confirmed entity received from the server.
// NOTE: we create Predicted here because it is used by multiple crates (prediction, replication)
#[derive(Debug, Reflect, Component)]
#[reflect(Component)]
pub struct Predicted {
    // This is an option because we could spawn pre-predicted entities on the client that exist before we receive
    // the corresponding confirmed entity
    pub confirmed_entity: Option<Entity>,
}
