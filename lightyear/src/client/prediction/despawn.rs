use bevy::ecs::system::EntityCommands;
use bevy::ecs::world::Command;
use bevy::prelude::{
    Commands, Component, DespawnRecursiveExt, Entity, OnRemove, Query, Reflect, ReflectComponent,
    Res, Trigger, With, Without, World,
};
use tracing::{debug, error, trace};

use crate::client::components::{ComponentSyncMode, Confirmed, SyncComponent};
use crate::client::config::ClientConfig;
use crate::client::prediction::Predicted;
use crate::prelude::{
    ComponentRegistry, Mode, PreSpawnedPlayerObject, ShouldBePredicted, TickManager,
};
use crate::shared::tick_manager::Tick;

// - TODO: despawning another client entity as a consequence from prediction, but we want to roll that back:
//   - maybe we don't do it, and we wait until we are sure (confirmed despawn) before actually despawning the entity

/// This command must be used to despawn the predicted or confirmed entity.
/// - If the entity is predicted, it can still be re-created if we realize during a rollback that it should not have been despawned.
pub struct PredictionDespawnCommand {
    entity: Entity,
}

#[derive(Component, PartialEq, Debug, Reflect)]
#[reflect(Component)]
pub(crate) struct PredictionDespawnMarker {
    // TODO: do we need this?
    death_tick: Tick,
}

impl Command for PredictionDespawnCommand {
    fn apply(self, world: &mut World) {
        let tick_manager = world.get_resource::<TickManager>().unwrap();
        let current_tick = tick_manager.tick();

        // if we are in host server mode, there is no rollback so we can despawn the entity immediately
        if world.resource::<ClientConfig>().shared.mode == Mode::HostServer {
            world.entity_mut(self.entity).despawn_recursive();
        }

        if let Ok(mut entity) = world.get_entity_mut(self.entity) {
            if entity.get::<Predicted>().is_some()
                || entity.get::<ShouldBePredicted>().is_some()
                // see https://github.com/cBournhonesque/lightyear/issues/818
                || entity.get::<PreSpawnedPlayerObject>().is_some()
            {
                // if this is a predicted or pre-predicted entity, do not despawn the entity immediately but instead
                // add a PredictionDespawn component to it to mark that it should be despawned as soon
                // as the confirmed entity catches up to it
                trace!("inserting prediction despawn marker");
                entity.insert(PredictionDespawnMarker {
                    // TODO: death_tick can be removed
                    //  - we can just wait until until the confirmed entity catches up and gets despawned as well
                    death_tick: current_tick,
                });
                // TODO: if we want the death to be immediate on predicted,
                //  we should despawn all components immediately (except Predicted and History)
            } else if let Some(confirmed) = entity.get::<Confirmed>() {
                // TODO: actually we should never despawn directly on the client a Confirmed entity
                //  it should only get despawned when replicating!
                entity.despawn_recursive();
            } else {
                error!("This command should only be called for predicted entities!");
            }
        }
    }
}

pub trait PredictionDespawnCommandsExt {
    fn prediction_despawn(&mut self);
}
impl PredictionDespawnCommandsExt for EntityCommands<'_> {
    fn prediction_despawn(&mut self) {
        let entity = self.id();
        self.commands().queue(PredictionDespawnCommand { entity })
    }
}

/// Despawn predicted entities when the confirmed entity gets despawned
pub(crate) fn despawn_confirmed(
    trigger: Trigger<OnRemove, Confirmed>,
    query: Query<&Confirmed>,
    mut commands: Commands,
) {
    if let Some(predicted) = query.get(trigger.entity()).unwrap().predicted {
        if let Some(entity_mut) = commands.get_entity(predicted) {
            entity_mut.despawn_recursive();
        }
    }
}

#[derive(Component)]
pub(crate) struct RemovedCache<C: Component>(pub Option<C>);

#[allow(clippy::type_complexity)]
/// Instead of despawning the entity, we remove all components except the history and the predicted marker
pub(crate) fn remove_component_for_despawn_predicted<C: SyncComponent>(
    component_registry: Res<ComponentRegistry>,
    mut commands: Commands,
    full_query: Query<Entity, (With<C>, With<PredictionDespawnMarker>)>,
    simple_query: Query<(Entity, &C), With<PredictionDespawnMarker>>,
) {
    match component_registry.prediction_mode::<C>() {
        // for full components, we can delete the component
        // it will get re-instated during rollback if the confirmed entity doesn't get despawned
        ComponentSyncMode::Full => {
            for entity in full_query.iter() {
                trace!("removing full component for prediction_despawn");
                commands.entity(entity).remove::<C>();
            }
        }
        // for simple/once components, there is no rollback, we can just cache them temporarily
        // and restore them in case of rollback
        ComponentSyncMode::Simple | ComponentSyncMode::Once => {
            for (entity, component) in simple_query.iter() {
                trace!("removing simple/once component for prediction_despawn");
                commands
                    .entity(entity)
                    .remove::<C>()
                    .insert(RemovedCache(Some(component.clone())));
            }
        }
        ComponentSyncMode::None => {}
    }
}

// TODO: compare the performance of cloning the component versus popping from the World directly
/// In case of a rollback, check if there were any entities that were predicted-despawn
/// that we need to re-instate. (all the entities that have RemovedCache<C> are in this scenario)
/// If we didn't need to re-instate them, the Confirmed entity would have been despawned.
///
/// Remember to reinstate components if SyncComponent != Full
pub(crate) fn restore_components_if_despawn_rolled_back<C: SyncComponent>(
    mut commands: Commands,
    mut query: Query<(Entity, &mut RemovedCache<C>), Without<C>>,
) {
    for (entity, mut cache) in query.iter_mut() {
        debug!("restoring component after rollback");
        let Some(component) = std::mem::take(&mut cache.0) else {
            debug!("could not find component");
            continue;
        };
        commands
            .entity(entity)
            .insert(component)
            .remove::<RemovedCache<C>>();
    }
}

/// Remove the despawn marker: if during rollback the entity are re-spawned, we don't want to re-despawn it again
/// PredictionDespawnMarker should only be present on the frame where we call `prediction_despawn`
pub(crate) fn remove_despawn_marker(
    mut commands: Commands,
    query: Query<Entity, With<PredictionDespawnMarker>>,
) {
    for entity in query.iter() {
        trace!("removing prediction despawn markerk");
        // SAFETY: bevy guarantees that the entity exists
        commands
            .get_entity(entity)
            .unwrap()
            .remove::<PredictionDespawnMarker>();
    }
}

#[cfg(test)]
mod tests {
    use crate::client::prediction::despawn::PredictionDespawnMarker;
    use crate::client::prediction::resource::PredictionManager;
    use crate::client::prediction::rollback::RollbackEvent;
    use crate::prelude::client::{Confirmed, PredictionDespawnCommandsExt};
    use crate::prelude::server::SyncTarget;
    use crate::prelude::{client, server, NetworkTarget};
    use crate::tests::protocol::{ComponentSyncModeFull, ComponentSyncModeSimple};
    use crate::tests::stepper::BevyStepper;
    use bevy::prelude::{default, Component, Trigger};

    #[derive(Component, Debug, PartialEq)]
    struct TestComponent(usize);

    /// Test that if a predicted entity gets despawned erroneously
    /// The rollback re-adds the predicted entity.
    #[test]
    fn test_despawned_predicted_rollback() {
        let mut stepper = BevyStepper::default();

        let server_entity = stepper
            .server_app
            .world_mut()
            .spawn((
                ComponentSyncModeFull(1.0),
                ComponentSyncModeSimple(1.0),
                server::Replicate {
                    sync: SyncTarget {
                        prediction: NetworkTarget::All,
                        ..default()
                    },
                    ..default()
                },
            ))
            .id();
        stepper.frame_step();
        stepper.frame_step();
        let confirmed_entity = stepper
            .client_app
            .world()
            .resource::<client::ConnectionManager>()
            .replication_receiver
            .remote_entity_map
            .get_local(server_entity)
            .expect("entity was not replicated to client");
        // check that prediction, interpolation, controlled was handled correctly
        let confirmed = stepper
            .client_app
            .world()
            .entity(confirmed_entity)
            .get::<Confirmed>()
            .expect("Confirmed component missing");
        let predicted_entity = confirmed.predicted.unwrap();
        // try adding a non-protocol component (which could be some rendering component)
        stepper
            .client_app
            .world_mut()
            .entity_mut(predicted_entity)
            .insert(TestComponent(1));

        // despawn the predicted entity locally
        stepper
            .client_app
            .world_mut()
            .commands()
            .entity(predicted_entity)
            .prediction_despawn();
        stepper.frame_step();
        // TODO: this does not work!
        // make sure that all components have been removed
        // assert!(stepper
        //     .client_app
        //     .world()
        //     .get::<TestComponent>(predicted_entity)
        //     .is_none());
        assert!(stepper
            .client_app
            .world()
            .get_entity(predicted_entity)
            .is_ok());
        assert!(stepper
            .client_app
            .world()
            .get::<ComponentSyncModeFull>(predicted_entity)
            .is_none());
        assert!(stepper
            .client_app
            .world()
            .get::<ComponentSyncModeSimple>(predicted_entity)
            .is_none());
        // the despawn marker should be removed immediately
        assert!(stepper
            .client_app
            .world()
            .get::<PredictionDespawnMarker>(predicted_entity)
            .is_none());

        // update the server entity to trigger a rollback where the predicted entity should be 're-spawned'
        stepper
            .server_app
            .world_mut()
            .get_mut::<ComponentSyncModeFull>(server_entity)
            .unwrap()
            .0 = 2.0;
        stepper.frame_step();
        stepper.frame_step();

        assert_eq!(
            stepper
                .client_app
                .world()
                .get::<ComponentSyncModeFull>(predicted_entity)
                .unwrap(),
            &ComponentSyncModeFull(2.0)
        );
        // TODO: NON-REGISTERED COMPONENTS DO NOT WORK!
        // the non-Full components should also get restored
        // assert_eq!(
        //     stepper
        //         .client_app
        //         .world()
        //         .get::<TestComponent>(predicted_entity)
        //         .unwrap(),
        //     &TestComponent(1)
        // );
        assert_eq!(
            stepper
                .client_app
                .world()
                .get::<ComponentSyncModeSimple>(predicted_entity)
                .unwrap(),
            &ComponentSyncModeSimple(1.0)
        );
    }

    /// Check that when the confirmed entity gets despawned, the predicted entity gets despawned as well
    #[test]
    fn test_despawned_confirmed() {
        let mut stepper = BevyStepper::default();

        let server_entity = stepper
            .server_app
            .world_mut()
            .spawn((
                ComponentSyncModeFull(1.0),
                ComponentSyncModeSimple(1.0),
                server::Replicate {
                    sync: SyncTarget {
                        prediction: NetworkTarget::All,
                        ..default()
                    },
                    ..default()
                },
            ))
            .id();
        stepper.frame_step();
        stepper.frame_step();
        let confirmed_entity = stepper
            .client_app
            .world()
            .resource::<client::ConnectionManager>()
            .replication_receiver
            .remote_entity_map
            .get_local(server_entity)
            .expect("entity was not replicated to client");
        // check that prediction, interpolation, controlled was handled correctly
        let confirmed = stepper
            .client_app
            .world()
            .entity(confirmed_entity)
            .get::<Confirmed>()
            .expect("Confirmed component missing");
        let predicted_entity = confirmed.predicted.unwrap();

        // despawn the confirmed entity
        stepper.client_app.world_mut().despawn(confirmed_entity);
        stepper.frame_step();

        // check that the predicted entity got despawned
        assert!(stepper
            .client_app
            .world()
            .get_entity(predicted_entity)
            .is_err());
        // check that the confirmed to predicted map got updated
        unsafe {
            assert!(stepper
                .client_app
                .world()
                .resource::<PredictionManager>()
                .predicted_entity_map
                .get()
                .as_ref()
                .unwrap()
                .confirmed_to_predicted
                .get(&confirmed_entity)
                .is_none());
        }
    }
}
