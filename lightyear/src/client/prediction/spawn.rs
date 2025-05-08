//! Logic to handle spawning Predicted entities
use bevy::prelude::{Added, Commands, Entity, Query, Res};
use tracing::debug;

use crate::client::components::Confirmed;
use crate::client::connection::ConnectionManager;
use crate::client::prediction::Predicted;
use crate::prelude::{ShouldBePredicted, TickManager};

/// Spawn a predicted entity for each confirmed entity that has the `ShouldBePredicted` component added
/// The `Confirmed` entity could already exist because we share the Confirmed component for prediction and interpolation.
// TODO: (although normally an entity shouldn't be both predicted and interpolated, so should we
//  instead panic if we find an entity that is both predicted and interpolated?)
pub(crate) fn spawn_predicted_entity(
    tick_manager: Res<TickManager>,
    connection: Res<ConnectionManager>,
    mut commands: Commands,

    // TODO: instead of listening to the ComponentInsertEvent, should we just directly query on Added<ShouldBePredicted>?
    //  maybe listening to the event is more performant, since Added<ShouldBePredicted> queries all entities that have this component?
    //  (which should actually be ok since we remove ShouldBePredicted immediately)
    //  But maybe this conflicts with PrePrediction and PreSpawning?
    //  Benchmark!
    // // get the list of entities who get ShouldBePredicted replicated from server
    // mut should_be_predicted_added: EventReader<ComponentInsertEvent<ShouldBePredicted>>,

    // only handle predicted that have ShouldBePredicted
    // (if the entity was handled by prespawn or prepredicted before, ShouldBePredicted gets removed)
    mut confirmed_entities: Query<(Entity, Option<&mut Confirmed>), Added<ShouldBePredicted>>,
) {
    for (confirmed_entity, confirmed) in confirmed_entities.iter_mut() {
        // skip if the entity already has a predicted entity
        if confirmed.as_ref().is_some_and(|c| c.predicted.is_some()) {
            continue;
        }
        debug!("Received entity with ShouldBePredicted from server: {confirmed_entity:?}");
        // we need to spawn a predicted entity for this confirmed entity
        let predicted_entity = commands
            .spawn(Predicted {
                confirmed_entity: Some(confirmed_entity),
            })
            .id();
        debug!(
            "Spawning predicted entity {:?} for confirmed: {:?}",
            predicted_entity, confirmed_entity
        );
        #[cfg(feature = "metrics")]
        {
            metrics::counter!("prediction::pre_predicted_spawn").increment(1);
        }

        // add Confirmed to the confirmed entity
        // safety: we know the entity exists
        let mut confirmed_entity_mut = commands.entity(confirmed_entity);
        confirmed_entity_mut.remove::<ShouldBePredicted>();
        if let Some(mut confirmed) = confirmed {
            confirmed.predicted = Some(predicted_entity);
        } else {
            // TODO: this is the same as the current tick no? or maybe not because we could have received updates before the spawn
            //  and they are applied simultaneously
            // get the confirmed tick for the entity
            // if we don't have it, something has gone very wrong
            let confirmed_tick = connection
                .replication_receiver
                .get_confirmed_tick(confirmed_entity)
                // in most cases we will have a confirmed tick. The only case where we don't is if
                // the entity was originally spawned on this client, but then authority was removed
                // and we not want to add Prediction
                .unwrap_or(tick_manager.tick());
            confirmed_entity_mut.insert(Confirmed {
                predicted: Some(predicted_entity),
                interpolated: None,
                tick: confirmed_tick,
            });
        }
    }
}

//
#[cfg(test)]
mod tests {
    use crate::client::components::Confirmed;
    use crate::prelude::server::SyncTarget;
    use crate::prelude::{client, server, ClientId, NetworkTarget};
    use crate::tests::stepper::{BevyStepper, TEST_CLIENT_ID};
    use bevy::ecs::hierarchy::ChildOf;
    use bevy::ecs::relationship::Relationship;
    use bevy::prelude::default;

    /// https://github.com/cBournhonesque/lightyear/issues/627
    /// Test that when we spawn a parent + child with hierarchy (ParentSync),
    /// the parent-child hierarchy is maintained on the predicted entities
    ///
    /// Flow:
    /// 1) Parent/Child get spawned on client
    /// 2) All components are inserted on child, including ParentSync (which is mapped correctly)
    ///    and ShouldBePredicted
    /// 3) In PredictionSet::Spawn, child-predicted is spawned, and Confirmed is added on child
    /// 4) Because Confirmed is added, we send an event to sync components from Confirmed to child-predicted
    ///    NOTE: we cannot sync the components at this point, because the parent-predicted entity is not spawned
    ///    so the ParentSync component cannot be mapped properly when it's synced to the child-predicted entity!
    ///
    /// We want to make sure that the order is
    /// "replicate-components -> spawn-prediction (for both child/parent) -> sync components (including ParentSync) -> update hierarchy"
    /// instead of
    /// "replicate-components -> spawn-prediction (for child) -> sync components (including ParentSync)
    ///   -> spawn-prediction (for parent) -> sync components -> update hierarchy"
    #[test]
    fn test_spawn_predicted_with_hierarchy() {
        let mut stepper = BevyStepper::default();

        let c = ClientId::Netcode(TEST_CLIENT_ID);
        let server_child = stepper.server_app.world_mut().spawn_empty().id();
        let server_parent = stepper
            .server_app
            .world_mut()
            .spawn(server::Replicate {
                sync: SyncTarget {
                    prediction: NetworkTarget::All,
                    ..default()
                },
                ..default()
            })
            .add_child(server_child)
            .id();
        stepper.frame_step();
        stepper.frame_step();
        stepper.frame_step();

        // check that the parent and child are spawned on the client
        let confirmed_child = stepper
            .client_app
            .world()
            .resource::<client::ConnectionManager>()
            .replication_receiver
            .remote_entity_map
            .get_local(server_child)
            .expect("child entity was not replicated to client");
        let confirmed_parent = stepper
            .client_app
            .world()
            .resource::<client::ConnectionManager>()
            .replication_receiver
            .remote_entity_map
            .get_local(server_parent)
            .expect("parent entity was not replicated to client");
        // dbg!(confirmed_child, confirmed_parent);

        // check that the parent-child hierarchy is maintained
        assert_eq!(
            stepper
                .client_app
                .world()
                .get::<ChildOf>(confirmed_child)
                .expect("confirmed child entity doesn't have a parent")
                .parent(),
            confirmed_parent
        );

        let predicted_child = stepper
            .client_app
            .world()
            .get::<Confirmed>(confirmed_child)
            .unwrap()
            .predicted
            .expect("confirmed child entity doesn't have a predicted entity");
        let predicted_parent = stepper
            .client_app
            .world()
            .get::<Confirmed>(confirmed_parent)
            .unwrap()
            .predicted
            .expect("confirmed parent entity doesn't have a predicted entity");

        // check that the parent-child hierarchy is present on the predicted entities
        assert_eq!(
            stepper
                .client_app
                .world()
                .get::<ChildOf>(predicted_child)
                .expect("predicted child entity doesn't have a parent")
                .parent(),
            predicted_parent
        );
    }
}
