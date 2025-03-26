use bevy::ecs::system::EntityCommands;
use bevy::prelude::*;
use tracing::{error, trace};

use crate::client::components::Confirmed;
use crate::client::prediction::Predicted;
use crate::prelude::{is_server_ref, AppIdentityExt, NetworkIdentityState, PreSpawned, ShouldBePredicted, TickManager};

/// This command must be used to despawn Predicted entities.
/// The reason is that we might want to not completely despawn the entity in case it gets 'restored' during a rollback.
/// (i.e. we do a rollback and we realize the entity should not have been despawned)
/// Instead we will Disable the entity so that it stops showing up.
///
/// The general flow is:
/// - we run predicted_despawn on the predicted entity
/// - `PredictedDespawnDisable` is added on the entity. We use our own custom marker instead of Disable in case users want to genuinely just
///   disable a Predicted entity.
/// - We can stop updating its PredictionHistory, or only update it with empty values (None)
/// - if the Confirmed entity is also despawned in the next few ticks, then the Predicted entity also gets despawned
/// - we still do rollback checks using the Confirmed updates against the `PredictedDespawn` entity! If there is a rollback,
///     we can remove the Disabled marker on all predicted entities, restore all their components to the Confirmed value, and then
///     re-run the last few-ticks (which might re-Disable the entity)
pub struct PredictionDespawnCommand {
    entity: Entity,
}

#[derive(Component, PartialEq, Debug, Reflect)]
#[reflect(Component)]
pub(crate) struct PredictionDisable;

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
                || entity.get::<PreSpawned>().is_some()
            {
                // if this is a predicted entity, do not despawn the entity immediately but instead
                // add a PredictionDisable component to it to mark it as disabled until the confirmed
                // entity catches up to it
                trace!("inserting prediction disable marker");
                entity.insert(PredictionDisable);
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
        self.queue(move |entity_mut: EntityWorldMut| {
            if is_server_ref(entity_mut.world().get_resource_ref::<State<NetworkIdentityState>>()) {
                entity_mut.despawn();
            } else {
                PredictionDespawnCommand { entity }.apply(entity_mut.into_world_mut());
            }
        });
    }
}

/// Despawn predicted entities when the confirmed entity gets despawned
pub(crate) fn despawn_confirmed(
    trigger: Trigger<OnRemove, Confirmed>,
    query: Query<&Confirmed>,
    mut commands: Commands,
) -> Result {
    if let Some(predicted) = query.get(trigger.target())?.predicted {
        if let Ok(mut entity_mut) = commands.get_entity(predicted) {
            entity_mut.despawn();
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::client::prediction::despawn::PredictionDisable;
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
        // check that a rollback occurred to add the components on the predicted entity
        assert_eq!(
            stepper
                .client_app
                .world()
                .get::<ComponentSyncModeFull>(predicted_entity)
                .unwrap(),
            &ComponentSyncModeFull(1.0)
        );
        assert_eq!(
            stepper
                .client_app
                .world()
                .get::<ComponentSyncModeSimple>(predicted_entity)
                .unwrap(),
            &ComponentSyncModeSimple(1.0)
        );
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
        // make sure that the entity is disabled
        assert!(stepper
            .client_app
            .world()
            .get_entity(predicted_entity)
            .is_ok());
        assert!(stepper
            .client_app
            .world()
            .get::<PredictionDisable>(predicted_entity)
            .is_some());

        // update the server entity to trigger a rollback where the predicted entity should be 're-spawned'
        stepper
            .server_app
            .world_mut()
            .get_mut::<ComponentSyncModeFull>(server_entity)
            .unwrap()
            .0 = 2.0;
        stepper.frame_step();
        stepper.frame_step();

        // Check that the entity was rolled back and the PredictionDisable marker was removed
        assert!(stepper
            .client_app
            .world()
            .get::<PredictionDisable>(predicted_entity)
            .is_none());
        assert_eq!(
            stepper
                .client_app
                .world()
                .get::<ComponentSyncModeFull>(predicted_entity)
                .unwrap(),
            &ComponentSyncModeFull(2.0)
        );
        // non-Full components are also present
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
