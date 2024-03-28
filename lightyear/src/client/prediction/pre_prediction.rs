//! Module to handle pre-prediction logic (entities that are created on the client first),
//! then the ownership gets transferred to the server.

use crate::client::components::Confirmed;
use crate::client::metadata::GlobalMetadata;
use crate::client::prediction::Predicted;
use crate::prelude::{Protocol, ShouldBePredicted};
use crate::shared::replication::components::{PrePredicted, Replicate};
use bevy::prelude::{Commands, Entity, Query, Res, With, Without};
use tracing::{debug, trace};

/// For pre-spawned entities, we want to stop replicating as soon as the initial spawn message has been sent to the
/// server. Otherwise any predicted action we would do affect the server entity, even though we want the server to
/// have authority on the entity.
/// Therefore we will remove the `Replicate` component right after the first time we've sent a replicating message to the
/// server
pub(crate) fn clean_pre_predicted_entity<P: Protocol>(
    mut commands: Commands,
    pre_predicted_entities: Query<Entity, (With<ShouldBePredicted>, Without<Confirmed>)>,
) {
    for entity in pre_predicted_entities.iter() {
        trace!(
            ?entity,
            "removing replicate from pre-spawned player-controlled entity"
        );
        commands
            .entity(entity)
            .remove::<Replicate<P>>()
            // don't remove should-be-predicted, so that we can know which entities were pre-predicted
            .remove::<ShouldBePredicted>()
            .insert((
                Predicted {
                    confirmed_entity: None,
                },
                // TODO: add this if we want to send inputs for pre-predicted entities before we receive the confirmed entity
                PrePredicted,
            ));
    }
}

// TODO: split pre-predicted from normally predicted, it's too confusing!!!

// TODO: should we run this only when Added<ShouldBePredicted>?
/// If a client adds `ShouldBePredicted` to an entity to perform pre-Prediction.
/// We automatically add the extra needed information to the component.
/// - client_entity: is needed to know which entity to use as the predicted entity
/// - client_id: is needed in case the pre-predicted entity is predicted by other players upon replication
pub(crate) fn handle_pre_prediction(
    metadata: Res<GlobalMetadata>,
    mut query: Query<(Entity, &mut ShouldBePredicted), Without<Confirmed>>,
) {
    for (entity, mut should_be_predicted) in query.iter_mut() {
        debug!(
            client_id = ?metadata.client_id.unwrap(),
            entity = ?entity,
            "adding pre-prediction info!");
        // TODO: actually we don't need to add the client_entity to the message.
        //  on the server, for pre-predictions, we can just use the entity that was sent in the message to set the value of ClientEntity.
        should_be_predicted.client_entity = Some(entity);
        should_be_predicted.client_id = Some(metadata.client_id.unwrap());
    }
}
