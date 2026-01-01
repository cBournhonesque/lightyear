use bevy_ecs::{component::Component, reflect::ReflectComponent};
use bevy_reflect::Reflect;

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
#[derive(Component, Clone, Copy, Debug, Default, Reflect)]
#[reflect(Component)]
pub struct Predicted;
