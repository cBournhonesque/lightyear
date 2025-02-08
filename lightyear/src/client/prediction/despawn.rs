use bevy::ecs::entity_disabling::Disabled;
use bevy::ecs::system::EntityCommands;
use bevy::prelude::*;
use tracing::{debug, error, trace};

use crate::client::components::{ComponentSyncMode, Confirmed, SyncComponent};
use crate::client::prediction::Predicted;
use crate::prelude::{
    AppIdentityExt, ComponentRegistry, PreSpawnedPlayerObject, ShouldBePredicted, TickManager,
};
use crate::shared::tick_manager::Tick;


/// This command must be used to despawn Predicted entities.
/// The reason is that we might want to not completely despawn the entity in case it gets 'restored' during a rollback.
/// (i.e. we do a rollback and we realize the entity should not have been despawned)
/// Instead we will Disable the entity so that it stops showing up.
///
/// The general flow is:
/// - we run predicted_despawn on the predicted entity
/// TODO: or make our own PredictedDisable marker!
/// - `Disabled` is added on the entity. (maybe also add a `PredictedDespawn` marker?). We can stop updating its PredictionHistory,
///    which will only contain empty values (None)
/// - if the Confirmed entity is also despawned in the next few ticks, then the Predicted entity also gets despawned
/// - we still do rollback checks using the Confirmed updates against the `PredictedDespawn` entity! If there is a rollback,
///     we can remove the Disabled marker on the predicted entity, restore all its components to the Confirmed value, and then
///     re-run the last few-ticks (which might re-Disable the entity)
pub struct PredictionDespawnCommand {
    entity: Entity,
}

#[derive(Component, PartialEq, Debug, Reflect)]
#[reflect(Component)]
pub(crate) struct PredictionDespawnMarker;

impl Command for PredictionDespawnCommand {
    fn apply(self, world: &mut World) {
        let tick_manager = world.get_resource::<TickManager>().unwrap();
        let current_tick = tick_manager.tick();

        // if we are in host server mode, there is no rollback so we can despawn the entity immediately
        if world.is_host_server() {
            world.entity_mut(self.entity).despawn();
            return;
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
                // mark the entity as Disabled so it stops showing up in queries
                entity.insert((PredictionDespawnMarker, Disabled));
                // TODO: if we want the death to be immediate on predicted,
                //  we should despawn all components immediately (except Predicted and History)
            } else if let Some(confirmed) = entity.get::<Confirmed>() {
                // TODO: actually we should never despawn directly on the client a Confirmed entity
                //  it should only get despawned when replicating!
                entity.despawn();
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
    if let Some(predicted) = query.get(trigger.target()).unwrap().predicted {
        if let Some(mut entity_mut) = commands.get_entity(predicted) {
            entity_mut.despawn();
        }
    }
}


/// Remove the despawn marker: if during rollback the entity are re-spawned, we don't want to re-despawn it again
/// PredictionDespawnMarker should only be present on the frame where we call `prediction_despawn`
pub(crate) fn remove_despawn_marker(
    mut commands: Commands,
    query: Query<Entity, (With<PredictionDespawnMarker>, With<Disabled>)>,
) {
    for entity in query.iter() {
        trace!("removing prediction despawn marker");
        commands
            .entity(entity)
            .remove::<PredictionDespawnMarker>();
    }
}

#[cfg(test)]
mod tests {
    use crate::client::prediction::despawn::PredictionDespawnMarker;
    use crate::client::prediction::resource::PredictionManager;
    use crate::prelude::client::{Confirmed, PredictionDespawnCommandsExt};
    use crate::prelude::server::SyncTarget;
    use crate::prelude::{client, server, NetworkTarget};
    use crate::tests::protocol::{ComponentSyncModeFull, ComponentSyncModeSimple};
    use crate::tests::stepper::BevyStepper;
    use bevy::prelude::{default, Component};

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
