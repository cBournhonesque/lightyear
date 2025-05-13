//! Module to handle pre-prediction logic (entities that are created on the client first),
//! then the ownership gets transferred to the server.

use crate::Predicted;
use crate::manager::{PredictionManager, PredictionResource};
use bevy::prelude::*;
use lightyear_connection::identity::{NetworkIdentityState, is_host_server};
use lightyear_core::prelude::{LocalTimeline, NetworkTimeline};
use lightyear_replication::prelude::{
    Confirmed, DisableReplicateHierarchy, Replicate, ReplicateLike, Replicating,
    ReplicationBufferSet, ReplicationGroup, ShouldBePredicted,
};
use serde::{Deserialize, Serialize};
use tracing::debug;

/// Indicates that an entity was pre-predicted
// NOTE: we do not map entities for this component, we want to receive the entities as is
//  because we already do the mapping at other steps
#[derive(Component, Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Reflect)]
#[reflect(Component)]
pub struct PrePredicted {
    pub(crate) confirmed_entity: Option<Entity>,
}

#[derive(Default)]
pub(crate) struct PrePredictionPlugin;

#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone, Copy)]
pub enum PrePredictionSet {
    // PostUpdate Sets
    /// Remove the Replicate component from pre-predicted entities (after replication)
    Clean,
}

impl Plugin for PrePredictionPlugin {
    fn build(&self, app: &mut App) {
        app.configure_sets(
            PostUpdate,
            (ReplicationBufferSet::Buffer, PrePredictionSet::Clean).chain(),
        );
        app.add_systems(
            PostUpdate,
            (
                // clean-up the ShouldBePredicted components after we've sent them
                Self::clean_pre_predicted_entity.in_set(PrePredictionSet::Clean),
            ), // .run_if(is_connected),
        );

        app.add_observer(Self::handle_prepredicted);
    }
}

impl PrePredictionPlugin {
    /// For pre-predicted entities, we want to stop replicating as soon as the initial spawn message has been sent to the
    /// server (to save bandwidth).
    /// The server will refuse those other updates anyway because it will take authority over the entity.
    /// Therefore we will remove the `Replicate` component right after the first time we've sent a replicating message to the
    /// server
    pub(crate) fn clean_pre_predicted_entity(
        mut commands: Commands,
        pre_predicted_entities: Query<Entity, (Added<PrePredicted>, Without<Confirmed>)>,
    ) {
        for entity in pre_predicted_entities.iter() {
            debug!(?entity, "removing replicate from pre-predicted entity");
            // remove Replicating first so that we don't replicate a despawn
            commands.entity(entity).remove::<Replicating>();
            commands.entity(entity).remove::<(
                Replicate,
                ReplicationGroup,
                DisableReplicateHierarchy,
                ReplicateLike,
            )>();
        }
    }

    /// When PrePredicted is added by the client: we spawn a Confirmed entity and update the mapping
    /// When PrePredicted is replicated from the server: we add the Predicted component
    pub(crate) fn handle_prepredicted(
        trigger: Trigger<OnAdd, PrePredicted>,
        mut commands: Commands,
        prediction_query: Single<&PredictionManager>,
        // TODO: should we fetch the value of PrePredicted to confirm that it matches what we expect?
    ) {
        let predicted_map = unsafe {
            prediction_query
                .predicted_entity_map
                .get()
                .as_ref()
                .unwrap()
        };
        // PrePredicted was replicated from the server:
        // When we receive an update from the server that confirms a pre-predicted entity,
        // we will add the Predicted component
        match predicted_map.confirmed_to_predicted.get(&trigger.target()) {
            Some(&predicted) => {
                let confirmed = trigger.target();
                debug!(
                    "Received PrePredicted entity from server. Confirmed: {confirmed:?}, Predicted: {predicted:?}"
                );
                commands.queue(move |world: &mut World| {
                    world
                        .entity_mut(predicted)
                        .insert(Predicted {
                            confirmed_entity: Some(confirmed),
                        })
                        .remove::<ShouldBePredicted>();
                });
            }
            _ => {
                let predicted_entity = trigger.target();
                let is_host_server = false;
                if is_host_server {
                    // for host-server, we don't want to spawn a separate entity because
                    //  the confirmed/predicted/server entity are the same! Instead we just want
                    //  to remove PrePredicted and add Predicted
                    commands.queue(move |world: &mut World| {
                        // world.entity_mut(predicted_entity).remove::<PrePredicted>();
                        world.entity_mut(predicted_entity).insert(Predicted {
                            confirmed_entity: Some(predicted_entity),
                        });
                    });
                } else {
                    // PrePredicted was added by the client:
                    // Spawn a Confirmed entity and update the mapping
                    commands.queue(move |world: &mut World| {
                        let Ok(timeline) = world.query::<&LocalTimeline>().single(world) else {
                            return;
                        };
                        let tick = timeline.tick();
                        let confirmed_entity = world
                            .spawn(Confirmed {
                                predicted: Some(predicted_entity),
                                interpolated: None,
                                tick,
                            })
                            .id();
                        debug!("Added PrePredicted on the client. Spawning confirmed entity: {confirmed_entity:?} for pre-predicted: {predicted_entity:?}");
                        world
                            .entity_mut(predicted_entity)
                            .get_mut::<PrePredicted>()
                            .unwrap()
                            .confirmed_entity = Some(confirmed_entity);
                        let manager_entity = world.resource::<PredictionResource>().link_entity;
                        world
                            .get_mut::<PredictionManager>(manager_entity)
                            .unwrap()
                            .predicted_entity_map
                            .get_mut()
                            .confirmed_to_predicted
                            .insert(confirmed_entity, predicted_entity);
                    });
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::predicted_history::PredictionHistory;
    use crate::prelude::server;
    use crate::prelude::server::AuthorityPeer;
    use crate::prelude::{ClientId, client};
    use crate::tests::host_server_stepper::HostServerStepper;
    use crate::tests::protocol::{ComponentClientToServer, PredictionModeFull};
    use crate::tests::stepper::{BevyStepper, TEST_CLIENT_ID};
    use bevy::ecs::relationship::Relationship;

    /// Simple preprediction case
    /// Also check that the PredictionHistory is correctly added to the PrePredicted entity
    #[test]
    fn test_pre_prediction() {
        // tracing_subscriber::FmtSubscriber::builder()
        //     .with_max_level(tracing::Level::DEBUG)
        //     .init();
        let mut stepper = BevyStepper::default();

        // spawn a pre-predicted entity on the client
        let predicted_entity = stepper
            .client_app
            .world_mut()
            .spawn((
                client::Replicate::default(),
                PredictionModeFull(1.0),
                PrePredicted::default(),
            ))
            .id();

        // flush to apply pre-predicted related commands
        stepper.flush();

        // check that the confirmed entity was spawned
        let confirmed_entity = stepper
            .client_app
            .world_mut()
            .query_filtered::<Entity, With<Confirmed>>()
            .single(stepper.client_app().world())
            .unwrap();

        // need to step multiple times because the server entity doesn't handle messages from future ticks
        for _ in 0..10 {
            stepper.frame_step();
        }

        // check that the server has received the entity
        // (we map from confirmed to server entity because the server updates the mapping upon reception)
        let server_entity = stepper
            .server_app
            .world()
            .resource::<server::ConnectionManager>()
            .connection(ClientId::Netcode(TEST_CLIENT_ID))
            .unwrap()
            .replication_receiver
            .remote_entity_map
            .get_local(confirmed_entity)
            .unwrap();

        // check that the authority has already been changed by the server to Server
        assert_eq!(
            stepper
                .server_app
                .world()
                .get::<AuthorityPeer>(server_entity)
                .unwrap(),
            &AuthorityPeer::Server
        );
        assert_eq!(
            stepper
                .server_app
                .world()
                .get::<PredictionModeFull>(server_entity)
                .unwrap()
                .0,
            1.0
        );

        // insert Replicate on the server entity
        stepper
            .server_app
            .world_mut()
            .entity_mut(server_entity)
            .insert(server::Replicate::default());

        stepper.flush();
        stepper.frame_step();
        stepper.frame_step();

        // it would be nice if the client's confirmed entity had a Replicated component
        assert!(
            stepper
                .client_app
                .world()
                .get::<HasAuthority>(confirmed_entity)
                .is_none()
        );
        stepper
            .server_app
            .world_mut()
            .get_mut::<PredictionModeFull>(server_entity)
            .unwrap()
            .0 = 2.0;

        stepper.frame_step();
        stepper.frame_step();
        assert_eq!(
            stepper
                .client_app
                .world()
                .get::<PredictionModeFull>(confirmed_entity)
                .unwrap()
                .0,
            2.0
        );
        assert!(
            stepper
                .client_app
                .world()
                .get::<PredictionHistory<PredictionModeFull>>(predicted_entity)
                .is_some()
        );
    }

    // TODO: test that pre-predicted works in host-server mode

    /// Test that PrePredicted works if ReplicateHierarchy is present.
    /// In that case, both the parent but also the children should be pre-predicted.
    #[test]
    fn test_pre_prediction_hierarchy() {
        // tracing_subscriber::FmtSubscriber::builder()
        //     .with_max_level(tracing::Level::DEBUG)
        //     .init();
        let mut stepper = BevyStepper::default();
        let child = stepper
            .client_app
            .world_mut()
            .spawn(PredictionModeFull(0.0))
            .id();
        let parent = stepper
            .client_app
            .world_mut()
            .spawn((
                ComponentClientToServer(0.0),
                client::Replicate::default(),
                PrePredicted::default(),
            ))
            .add_child(child)
            .id();

        for _ in 0..10 {
            stepper.frame_step();
        }

        // check that PrePredicted was also added on the child
        assert!(
            stepper
                .client_app
                .world()
                .get::<PrePredicted>(child)
                .is_some()
        );

        // check that both the parent and the child were replicated
        let server_parent = stepper
            .server_app
            .world_mut()
            .query_filtered::<Entity, With<ComponentClientToServer>>()
            .single(stepper.server_app.world())
            .expect("parent entity was not replicated");
        let server_child = stepper
            .server_app
            .world_mut()
            .query_filtered::<Entity, With<PredictionModeFull>>()
            .single(stepper.server_app.world())
            .expect("child entity was not replicated");
        assert_eq!(
            stepper
                .server_app
                .world()
                .get::<ChildOf>(server_child)
                .unwrap()
                .get(),
            server_parent
        );

        // add Replicate on the server side
        stepper
            .server_app
            .world_mut()
            .entity_mut(server_parent)
            .insert(server::Replicate::default());

        stepper.frame_step();
        stepper.frame_step();

        // check that the client parent and child entity both have the Predicted component, and that a confirmed entity has been spawned
        let parent_predicted = stepper
            .client_app()
            .world()
            .get::<Predicted>(parent)
            .unwrap();
        let confirmed_entity = parent_predicted.confirmed_entity.unwrap();
        assert!(
            stepper
                .client_app
                .world()
                .get::<Confirmed>(confirmed_entity)
                .is_some()
        );

        let child_predicted = stepper
            .client_app()
            .world()
            .get::<Predicted>(child)
            .unwrap();
        let confirmed_entity = child_predicted.confirmed_entity.unwrap();
        assert!(
            stepper
                .client_app
                .world()
                .get::<Confirmed>(confirmed_entity)
                .is_some()
        );
    }

    #[test]
    fn test_pre_prediction_host_server() {
        let mut stepper = HostServerStepper::default();

        // spawn a pre-predicted entity on the client
        let predicted_entity = stepper
            .server_app
            .world_mut()
            .spawn((
                client::Replicate::default(),
                PredictionModeFull(1.0),
                PrePredicted::default(),
            ))
            .id();

        stepper.frame_step();

        // since we're running in host-stepper mode, the Predicted component should also have been added
        // (but not Confirmed)
        let confirmed_entity = stepper
            .server_app
            .world_mut()
            .query_filtered::<Entity, With<Predicted>>()
            .single(stepper.server_app.world())
            .unwrap();

        // need to step multiple times because the server entity doesn't handle messages from future ticks
        for _ in 0..10 {
            stepper.frame_step();
        }
    }
}
