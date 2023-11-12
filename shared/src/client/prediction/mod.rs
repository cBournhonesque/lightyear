pub use crate::replication::prediction::ShouldBePredicted;
pub use plugin::add_prediction_systems;

use crate::tick::Tick;
use bevy::prelude::{Added, Commands, Component, Entity, Query, Resource};

/// This file is dedicated to running Prediction on entities.
/// On the client side, we run prediction on entities that are owned by the client.
/// The server is on tick 5, client is on tick 10, but we already applied the user's inputs for ticks 6->10.
///
/// When the server messages arrives (for tick 0), we:
/// - copy the server state (player movement, etc.) into the client's state
/// - reapply the last 10 frames, and re-apply the user's inputs in those 10 frames
/// the received server state. (server reconciliation)

/// Which means that for each predicted entity, we need:
/// - a buffer of the client inputs for the last RTT ticks
/// - a buffer of the components' states for the last RTT Ticks?  -> TO CHECK IF THERE WAS A MISPREDICTION AND WE NEED TO REPREDICT
/// - list of all the components that will be re-computed for reconciliation

/// Marks an entity that is being predicted by the client
#[derive(Component)]
pub struct Predicted {
    confirmed_entity: Entity,
    // rollback_state: RollbackState,
}

// TODO: don't use atomic yet because multi-threading is not enabled in wasm. + we need to benchmark first ?
#[derive(Resource)]
pub struct Rollback {
    pub(crate) state: RollbackState,
}

/// Resource that will track whether we should do rollback or not
/// (We have this as a resource because if any predicted entity needs to be rolled-back; we should roll back all predicted entities)
// #[atomic_enum]
#[derive(Debug)]
pub enum RollbackState {
    Default,
    ShouldRollback {
        // tick we are setting (to record history)k
        current_tick: Tick,
    },
    DidRollback,
}

/// Marks an entity that contains the server-updates that are received from the Server
/// (this entity is a copy of Predicted that is RTT ticks behind)
#[derive(Component)]
pub struct Confirmed {
    predicted: Entity,
}

/// ROLLBACK INSERT: have to do rollback

/// ROLLBACK SPAWN

/// ROLLBACK UPDATE:
/// WHEN WE RECEIVE PACKETS FROM SERVER
///
/// We receive packets from the server. The packet from the server will include the latest tick that the server has processed.
/// In a given client render-frame, We might receive server packets for different components of the same entity, but with different server ticks.
/// For each of these components, we compare against what we had in our recorded history.
///
/// The Confirmed entity has all latest updates applied
///
/// 2 options:
/// - for each component, we compare the predicted history at the update tick for that component with the confirmed entity (server's version of the component at the update tick).
///   if mismatch, we must rollback at least from that tick. We rollback from the earliest tick across all components
/// - for each component, we compare the predicted history at the latest server tick received across the confirmed entity (server's version at latest server tick).
///   if mismatch, we must rollback to the latest server tick.
///
/// - one solution would be to include all the component updates for a given entity in the same message. (which should be what is happening? we are aggregating all updates).
/// let's go with option 2 then
///
///
/// If we need to rollback, currently we only rollback the predicted entity.
/// TODO: Maybe in the future, we should instead rollback ALL predicted entities ? (similar to rocket league)

/// Component that is predicted by the client
// #[bevy_trait_query::queryable]
pub trait PredictedComponent: Component + Clone + PartialEq + Eq {}

pub trait A {}

pub mod plugin;
mod predicted_history;
pub(crate) mod rollback;

// What we want to achieve:
// - client defines a set of components that are predicted.
// - several cases:
//    - not owner prediction: we spawn ball on server, we choose on server to add [Confirmed] component.
//      Confirmed gets replicated, we spawn a predicted ball on client for the last server tick, we quickly fast-forward it via rollback?
//    - owner prediction: we spawn player on server, we choose on server to add [Confirmed] component.
//      Confirmed gets replicated, we spawn a predicted player on client for the last server tick, we quickly fast-forward it with rollback (and apply buffer inputs)
//
//  - other approach:
//    - we know on the client which entity to predict (for example ball + player), we spawn the predicted on client right away. seems worse.
//
// - what's annoying is that Confirmed contains some client-specific information that will get replicated. Maybe we can create a specific ShouldBeReplicated marker for this.
// for now, the easiest option would be to just replicate the entirety of Confirmed ?
pub fn spawn_predicted_entity(
    mut commands: Commands,
    confirmed_entities: Query<Entity, Added<ShouldBePredicted>>,
) {
    for confirmed_entity in confirmed_entities.iter() {
        // spawn a new predicted entity
        let predicted_entity_mut = commands.spawn((Predicted { confirmed_entity }));
        let predicted_entity = predicted_entity_mut.id();
        // add Confirmed to the confirmed entity
        // safety: we know the entity exists
        let mut confirmed_entity_mut = commands.get_entity(confirmed_entity).unwrap();
        confirmed_entity_mut.insert(
            (Confirmed {
                predicted: predicted_entity,
            }),
        );
    }
}
