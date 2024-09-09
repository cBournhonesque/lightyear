use bevy::ecs::system::EntityCommands;
use bevy::ecs::world::Command;
use bevy::prelude::{
    Commands, Component, DespawnRecursiveExt, Entity, OnRemove, Query, Reflect, ReflectComponent,
    Res, ResMut, Trigger, With, Without, World,
};
use tracing::{debug, error, trace};

use crate::client::components::{ComponentSyncMode, Confirmed, SyncComponent};
use crate::client::config::ClientConfig;
use crate::client::prediction::resource::PredictionManager;
use crate::client::prediction::Predicted;
use crate::prelude::{ComponentRegistry, Mode, ShouldBePredicted, TickManager};
use crate::shared::tick_manager::Tick;

// - TODO: despawning another client entity as a consequence from prediction, but we want to roll that back:
//   - maybe we don't do it, and we wait until we are sure (confirmed despawn) before actually despawning the entity

/// This command must be used to despawn the predicted or confirmed entity.
/// - If the entity is predicted, it can still be re-created if we realize during a rollback that it should not have been despawned.
/// - If the entity is confirmed, we despawn both the predicted and confirmed entities
pub struct PredictionDespawnCommand {
    entity: Entity,
}

#[derive(Component, PartialEq, Debug, Reflect)]
#[reflect(Component)]
pub(crate) struct PredictionDespawnMarker {
    // TODO: do we need this?
    // TODO: it's pub just for integration tests right now
    pub(crate) death_tick: Tick,
}

impl Command for PredictionDespawnCommand {
    fn apply(self, world: &mut World) {
        let tick_manager = world.get_resource::<TickManager>().unwrap();
        let current_tick = tick_manager.tick();

        // if we are in host server mode, there is no rollback so we can despawn the entity immediately
        if world.resource::<ClientConfig>().shared.mode == Mode::HostServer {
            world.despawn(self.entity);
        }

        let mut predicted_entity_to_despawn: Option<Entity> = None;

        if let Some(mut entity) = world.get_entity_mut(self.entity) {
            if entity.get::<Predicted>().is_some() || entity.get::<ShouldBePredicted>().is_some() {
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

                // if this is a confirmed entity
                // despawn both predicted and confirmed
                if let Some(predicted) = confirmed.predicted {
                    predicted_entity_to_despawn = Some(predicted);
                }
                entity.despawn();
            } else {
                error!("This command should only be called for predicted entities!");
            }
        }
        if let Some(entity) = predicted_entity_to_despawn {
            world.despawn(entity);
        }
    }
}

pub trait PredictionDespawnCommandsExt {
    fn prediction_despawn(&mut self);
}
impl PredictionDespawnCommandsExt for EntityCommands<'_> {
    fn prediction_despawn(&mut self) {
        let entity = self.id();
        self.commands().add(PredictionDespawnCommand { entity })
    }
}

/// Despawn predicted entities when the confirmed entity gets despawned
pub(crate) fn despawn_confirmed(
    trigger: Trigger<OnRemove, Confirmed>,
    mut manager: ResMut<PredictionManager>,
    mut commands: Commands,
) {
    if let Some(predicted) = manager
        .predicted_entity_map
        .get_mut()
        .confirmed_to_predicted
        .remove(&trigger.entity())
    {
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
/// In case we rollback the despawn, we need to restore the removed components
/// even if those components are not checking for rollback (SyncComponent != Full)
/// For those components, we just re-add them from the cache at the start of rollback
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

// /// In case we rollback the despawn, we need to restore the removed components
// /// even if those components are not checking for rollback (SyncComponent != Full)
// /// For those components, we just re-add them from the cache at the start of rollback
// pub(crate) fn restore_components_if_despawn_rolled_back<C: SyncComponent>(world: &mut World) {
//     for (entity, cache) in world
//         .query_filtered::<(Entity, &RemovedCache<C>), Without<C>>()
//         .iter(&world)
//     {
//         trace!("restoring component after rollback");
//         let mut entity_mut = world.entity_mut(entity);
//         if let Some(c) = entity_mut.take::<RemovedCache<C>>() {
//             entity_mut.insert(c.0);
//         }
//         // .remove::<RemovedCache<C>>
//         // commands
//         //     .entity(entity)
//         //     .insert(cache.0.clone())
//         //     .remove::<RemovedCache<C>>();
//     }
// }

/// Remove the despawn marker: if during rollback the components are re-spawned, we don't want to re-despawn them again
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

// TODO: revisit this; rollbacks happen when we receive a replication message now
// #[cfg(test)]
// mod tests {
//     use crate::_reexport::*;
//     use crate::prelude::client::*;
//     use crate::prelude::*;
//     use crate::tests::protocol::*;
//     use crate::tests::stepper::{BevyStepper};
//     use bevy::prelude::*;
//     use bevy::utils::Duration;
//
//     fn increment_component_and_despawn(
//         mut commands: Commands,
//         mut query: Query<(Entity, &mut Component1), With<Predicted>>,
//     ) {
//         for (entity, mut component) in query.iter_mut() {
//             component.0 += 1.0;
//             if component.0 == 5.0 {
//                 commands.entity(entity).prediction_despawn::<MyProtocol>();
//             }
//         }
//     }
//
//     // Test that if a predicted entity gets despawned erroneously
//     // We are still able to rollback properly (the rollback re-adds the predicted entity, or prevents it from despawning)
//     #[test]
//     fn test_despawned_predicted_rollback() -> anyhow::Result<()> {
//         let frame_duration = Duration::from_millis(10);
//         let tick_duration = Duration::from_millis(10);
//         let shared_config = SharedConfig {
//             enable_replication: false,
//             tick: TickConfig::new(tick_duration),
//             ..Default::default()
//         };
//         let link_conditioner = LinkConditionerConfig {
//             incoming_latency: Duration::from_millis(40),
//             incoming_jitter: Duration::from_millis(5),
//             incoming_loss: 0.05,
//         };
//         let sync_config = SyncConfig::default().speedup_factor(1.0);
//         let prediction_config = PredictionConfig::default();
//         let interpolation_delay = Duration::from_millis(100);
//         let interpolation_config = InterpolationConfig::default().with_delay(InterpolationDelay {
//             min_delay: interpolation_delay,
//             send_interval_ratio: 0.0,
//         });
//         let mut stepper = BevyStepper::new(
//             shared_config,
//             sync_config,
//             prediction_config,
//             interpolation_config,
//             link_conditioner,
//             frame_duration,
//         );
//         stepper.client_mut().set_synced();
//         stepper.client_app.add_systems(
//             FixedUpdate,
//             increment_component_and_despawn
//         );
//
//         // Create a confirmed entity
//         let confirmed = stepper
//             .client_app
//             .world_mut()
//             .spawn((Component1(0.0), ShouldBePredicted))
//             .id();
//
//         // Tick once
//         stepper.frame_step();
//         assert_eq!(stepper.client().tick(), Tick(1));
//         let predicted = stepper
//             .client_app
//             .world()//             .get::<Confirmed>(confirmed)
//             .unwrap()
//             .predicted
//             .unwrap();
//
//         // check that the predicted entity got spawned
//         assert_eq!(
//             stepper
//                 .client_app
//                 .world()//                 .get::<Predicted>(predicted)
//                 .unwrap()
//                 .confirmed_entity,
//             confirmed
//         );
//
//         // check that the component history got created
//         let mut history = PredictionHistory::<Component1>::default();
//         history
//             .buffer
//             .add_item(Tick(0), ComponentState::Updated(Component1(0.0)));
//         history
//             .buffer
//             .add_item(Tick(1), ComponentState::Updated(Component1(1.0)));
//         assert_eq!(
//             stepper
//                 .client_app
//                 .world()//                 .get::<PredictionHistory<Component1>>(predicted)
//                 .unwrap(),
//             &history,
//         );
//         // check that the confirmed component got replicated
//         assert_eq!(
//             stepper
//                 .client_app
//                 .world()//                 .get::<Component1>(predicted)
//                 .unwrap(),
//             &Component1(1.0)
//         );
//
//         // advance five more frames, so that the component gets removed on predicted
//         for i in 0..5 {
//             stepper.frame_step();
//         }
//         assert_eq!(stepper.client().tick(), Tick(6));
//
//         // check that the component got removed on predicted
//         assert!(stepper
//             .client_app
//             .world()//             .get::<Component1>(predicted)
//             .is_none());
//         // // check that predicted has the despawn marker
//         // assert_eq!(
//         //     stepper
//         //         .client_app
//         //         .world()//         //         .get::<PredictionDespawnMarker>(predicted)
//         //         .unwrap(),
//         //     &PredictionDespawnMarker {
//         //         death_tick: Tick(5)
//         //     }
//         // );
//         // check that the component history is still there and that the value of the component history is correct
//         let mut history = PredictionHistory::<Component1>::default();
//         for i in 0..5 {
//             history
//                 .buffer
//                 .add_item(Tick(i), ComponentState::Updated(Component1(i as f32)));
//         }
//         history.buffer.add_item(Tick(5), ComponentState::Removed);
//         assert_eq!(
//             stepper
//                 .client_app
//                 .world()//                 .get::<PredictionHistory<Component1>>(predicted)
//                 .unwrap(),
//             &history,
//         );
//
//         // create a rollback situation
//         stepper.client_mut().set_synced();
//         stepper
//             .client_mut()
//             .set_latest_received_server_tick(Tick(3));
//         stepper
//             .client_app
//             .world_mut()
//             .get_mut::<Component1>(confirmed)
//             .unwrap()
//             .0 = 1.0;
//         // update without incrementing time, because we want to force a rollback check
//         stepper.client_app.update();
//
//         // check that rollback happened
//         // predicted exists, and got the component re-added
//         stepper
//             .client_app
//             .world_mut()
//             .get_mut::<Component1>(predicted)
//             .unwrap()
//             .0 = 4.0;
//         // check that the history is how we expect after rollback
//         let mut history = PredictionHistory::<Component1>::default();
//         for i in 3..7 {
//             history
//                 .buffer
//                 .add_item(Tick(i), ComponentState::Updated(Component1(i as f32 - 2.0)));
//         }
//         assert_eq!(
//             stepper
//                 .client_app
//                 .world()//                 .get::<PredictionHistory<Component1>>(predicted)
//                 .unwrap(),
//             &history
//         );
//         Ok(())
//     }
//
//     // Test that if another entity gets added during prediction,
//     // - either it should get despawned if there is a rollback that doesn't add it anymore
//     // - or we should just let it live? (imagine it's audio, etc.)
//
//     fn increment_component_and_despawn_both(
//         mut commands: Commands,
//         mut query: Query<(Entity, &mut Component1)>,
//     ) {
//         for (entity, mut component) in query.iter_mut() {
//             component.0 += 1.0;
//             if component.0 == 5.0 {
//                 commands.entity(entity).prediction_despawn::<MyProtocol>();
//             }
//         }
//     }
//
//     // Test that if a confirmed entity gets despawned,
//     // the corresponding predicted entity gets despawned as well
//     // Test that if a predicted entity gets despawned erroneously
//     // We are still able to rollback properly (the rollback re-adds the predicted entity, or prevents it from despawning)
//     #[test]
//     fn test_despawned_confirmed_rollback() -> anyhow::Result<()> {
//         let frame_duration = Duration::from_millis(10);
//         let tick_duration = Duration::from_millis(10);
//         let shared_config = SharedConfig {
//             enable_replication: false,
//             tick: TickConfig::new(tick_duration),
//             ..Default::default()
//         };
//         let link_conditioner = LinkConditionerConfig {
//             incoming_latency: Duration::from_millis(40),
//             incoming_jitter: Duration::from_millis(5),
//             incoming_loss: 0.05,
//         };
//         let sync_config = SyncConfig::default().speedup_factor(1.0);
//         let prediction_config = PredictionConfig::default();
//         let interpolation_delay = Duration::from_millis(100);
//         let interpolation_config = InterpolationConfig::default().with_delay(InterpolationDelay {
//             min_delay: interpolation_delay,
//             send_interval_ratio: 0.0,
//         });
//         let mut stepper = BevyStepper::new(
//             shared_config,
//             sync_config,
//             prediction_config,
//             interpolation_config,
//             link_conditioner,
//             frame_duration,
//         );
//         stepper.client_app.add_systems(
//             FixedUpdate,
//             increment_component_and_despawn_both
//         );
//
//         // Create a confirmed entity
//         let confirmed = stepper
//             .client_app
//             .world_mut()
//             .spawn((Component1(0.0), ShouldBePredicted))
//             .id();
//
//         // Tick once
//         stepper.frame_step();
//         assert_eq!(stepper.client().tick(), Tick(1));
//         let predicted = stepper
//             .client_app
//             .world()//             .get::<Confirmed>(confirmed)
//             .unwrap()
//             .predicted
//             .unwrap();
//
//         // check that the predicted entity got spawned
//         assert_eq!(
//             stepper
//                 .client_app
//                 .world()//                 .get::<Predicted>(predicted)
//                 .unwrap()
//                 .confirmed_entity,
//             confirmed
//         );
//
//         // check that the component history got created
//         let mut history = PredictionHistory::<Component1>::default();
//         history
//             .buffer
//             .add_item(Tick(1), ComponentState::Updated(Component1(1.0)));
//         assert_eq!(
//             stepper
//                 .client_app
//                 .world()//                 .get::<PredictionHistory<Component1>>(predicted)
//                 .unwrap(),
//             &history,
//         );
//         // check that the confirmed component got replicated
//         assert_eq!(
//             stepper
//                 .client_app
//                 .world()//                 .get::<Component1>(predicted)
//                 .unwrap(),
//             &Component1(1.0)
//         );
//
//         // create a situation where the confirmed entity gets despawned during FixedUpdate::Main
//         stepper.client_mut().set_synced();
//         stepper
//             .client_mut()
//             .set_latest_received_server_tick(Tick(0));
//         // we set it to 5 so that it gets despawned during FixedUpdate::Main
//         stepper
//             .client_app
//             .world_mut()
//             .get_mut::<Component1>(confirmed)
//             .unwrap()
//             .0 = 4.0;
//         // update without incrementing time, because we want to force a rollback check
//         stepper.frame_step();
//
//         // check that rollback happened
//         // confirmed and predicted both got despawned
//         assert!(stepper.client_app.world().get_entity(confirmed).is_none());
//         assert!(stepper.client_app.world().get_entity(predicted).is_none());
//
//         Ok(())
//     }
// }
