use std::marker::PhantomData;

use bevy::ecs::system::{Command, EntityCommands};
use bevy::prelude::{Commands, Component, Entity, Query, With, World};

use crate::client::components::{Confirmed, SyncComponent};
use crate::client::prediction::Predicted;
use crate::client::resource::Client;
use crate::protocol::Protocol;
use crate::shared::tick_manager::Tick;

// Despawn logic:
// - despawning a predicted client entity:
//   - we add a DespawnMarker component to the entity
//   - all components other than ComponentHistory or Predicted get despawned, so that we can still check for rollbacks
//   - if the confirmed entity gets despawned, we despawn the predicted entity
//   - if the confirmed entity doesn't get despawned (during rollback, for example), it will re-add the necessary components to the predicted entity

// - TODO: despawning another client entity as a consequence from prediction, but we want to roll that back:
//   - maybe we don't do it, and we wait until we are sure (confirmed despawn) before actually despawning the entity

pub trait PredictionCommandsExt {
    fn prediction_despawn<P: Protocol>(&mut self);
}

/// This command must be used to despawn the predicted or confirmed entity.
/// - If the entity is predicted, it can still be re-created if we realize during a rollback that it should not have been despawned.
/// - If the entity is confirmed, we despawn both the predicted and confirmed entities
pub struct PredictionDespawnCommand<P: Protocol> {
    entity: Entity,
    _marker: PhantomData<P>,
}

#[derive(Component, PartialEq, Debug)]
pub struct PredictionDespawnMarker {
    // TODO: do we need this?
    // TODO: it's pub just for integration tests right now
    pub death_tick: Tick,
}

impl<P: Protocol> Command for PredictionDespawnCommand<P> {
    fn apply(self, world: &mut World) {
        let client = world.get_resource::<Client<P>>().unwrap();
        let current_tick = client.tick();

        let mut predicted_entity_to_despawn: Option<Entity> = None;

        if let Some(mut entity) = world.get_entity_mut(self.entity) {
            if entity.get::<Predicted>().is_some() {
                // if this is a predicted entity, do not despawn the entity immediately but instead
                // add a PredictionDespawn component to it to mark that it should be despawned as soon
                // as the confirmed entity catches up to it
                entity.insert(PredictionDespawnMarker {
                    death_tick: current_tick,
                });
                // TODO: if we want the death to be immediate on predicted,
                //  we should despawn all components immediately (except Predicted and History)
            } else if let Some(confirmed) = entity.get::<Confirmed>() {
                // if this is a confirmed entity
                // despawn both predicted and confirmed
                if let Some(predicted) = confirmed.predicted {
                    predicted_entity_to_despawn = Some(predicted);
                }
                entity.despawn();
            } else {
                panic!("this command should only be called for predicted or confirmed entities");
            }
        }
        if let Some(entity) = predicted_entity_to_despawn {
            world.despawn(entity);
        }
    }
}

impl PredictionCommandsExt for EntityCommands<'_, '_, '_> {
    fn prediction_despawn<P: Protocol>(&mut self) {
        let entity = self.id();
        self.commands().add(PredictionDespawnCommand {
            entity,
            _marker: PhantomData::<P>,
        })
    }
}

#[allow(clippy::type_complexity)]
pub(crate) fn remove_component_for_despawn_predicted<C: SyncComponent>(
    mut commands: Commands,
    query: Query<Entity, (With<C>, With<Predicted>, With<PredictionDespawnMarker>)>,
) {
    for entity in query.iter() {
        // SAFETY: bevy guarantees that the entity exists
        commands.get_entity(entity).unwrap().remove::<C>();
    }
}

/// Remove the despawn marker: if during rollback the components are re-spawned, we don't want to re-despawn them again
pub(crate) fn remove_despawn_marker(
    mut commands: Commands,
    query: Query<Entity, With<PredictionDespawnMarker>>,
) {
    for entity in query.iter() {
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
//     use crate::tests::stepper::{BevyStepper, Step};
//     use bevy::prelude::*;
//     use std::time::Duration;
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
//         let prediction_config = PredictionConfig::default().disable(false);
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
//             increment_component_and_despawn.in_set(FixedUpdateSet::Main),
//         );
//
//         // Create a confirmed entity
//         let confirmed = stepper
//             .client_app
//             .world
//             .spawn((Component1(0.0), ShouldBePredicted))
//             .id();
//
//         // Tick once
//         stepper.frame_step();
//         assert_eq!(stepper.client().tick(), Tick(1));
//         let predicted = stepper
//             .client_app
//             .world
//             .get::<Confirmed>(confirmed)
//             .unwrap()
//             .predicted
//             .unwrap();
//
//         // check that the predicted entity got spawned
//         assert_eq!(
//             stepper
//                 .client_app
//                 .world
//                 .get::<Predicted>(predicted)
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
//                 .world
//                 .get::<PredictionHistory<Component1>>(predicted)
//                 .unwrap(),
//             &history,
//         );
//         // check that the confirmed component got replicated
//         assert_eq!(
//             stepper
//                 .client_app
//                 .world
//                 .get::<Component1>(predicted)
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
//             .world
//             .get::<Component1>(predicted)
//             .is_none());
//         // // check that predicted has the despawn marker
//         // assert_eq!(
//         //     stepper
//         //         .client_app
//         //         .world
//         //         .get::<PredictionDespawnMarker>(predicted)
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
//                 .world
//                 .get::<PredictionHistory<Component1>>(predicted)
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
//             .world
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
//             .world
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
//                 .world
//                 .get::<PredictionHistory<Component1>>(predicted)
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
//         let prediction_config = PredictionConfig::default().disable(false);
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
//             increment_component_and_despawn_both.in_set(FixedUpdateSet::Main),
//         );
//
//         // Create a confirmed entity
//         let confirmed = stepper
//             .client_app
//             .world
//             .spawn((Component1(0.0), ShouldBePredicted))
//             .id();
//
//         // Tick once
//         stepper.frame_step();
//         assert_eq!(stepper.client().tick(), Tick(1));
//         let predicted = stepper
//             .client_app
//             .world
//             .get::<Confirmed>(confirmed)
//             .unwrap()
//             .predicted
//             .unwrap();
//
//         // check that the predicted entity got spawned
//         assert_eq!(
//             stepper
//                 .client_app
//                 .world
//                 .get::<Predicted>(predicted)
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
//                 .world
//                 .get::<PredictionHistory<Component1>>(predicted)
//                 .unwrap(),
//             &history,
//         );
//         // check that the confirmed component got replicated
//         assert_eq!(
//             stepper
//                 .client_app
//                 .world
//                 .get::<Component1>(predicted)
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
//             .world
//             .get_mut::<Component1>(confirmed)
//             .unwrap()
//             .0 = 4.0;
//         // update without incrementing time, because we want to force a rollback check
//         stepper.frame_step();
//
//         // check that rollback happened
//         // confirmed and predicted both got despawned
//         assert!(stepper.client_app.world.get_entity(confirmed).is_none());
//         assert!(stepper.client_app.world.get_entity(predicted).is_none());
//
//         Ok(())
//     }
// }
