//! Handles spawning entities that are predicted
use bevy::ecs::component::Components;
use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use tracing::{debug, trace};

use crate::client::components::Confirmed;
use crate::client::connection::ConnectionManager;
use crate::client::events::ComponentInsertEvent;
use crate::client::prediction::resource::PredictionManager;
use crate::client::prediction::rollback::Rollback;
use crate::client::prediction::Predicted;
use crate::prelude::client::PredictionSet;
use crate::prelude::{ComponentRegistry, Replicated, ShouldBePredicted, TickManager};

use crate::shared::replication::prespawn::compute_default_hash;
use crate::shared::sets::{ClientMarker, InternalReplicationSet};

#[derive(Default)]
pub(crate) struct PreSpawnedPlayerObjectPlugin;

#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone, Copy)]
pub enum PreSpawnedPlayerObjectSet {
    // PreUpdate Sets
    /// When we receive an entity from the server that contains the [`PreSpawnedPlayerObject`] component,
    /// that means that it was already spawned on the client.
    /// Do the matching process to find the corresponding client entity
    Spawn,
    // PostUpdate Sets
    /// Add the necessary information to the PrePrediction component (before replication)
    /// Clean up the PreSpawnedPlayerObject entities for which we couldn't find a mapped server entity
    CleanUp,
}

impl Plugin for PreSpawnedPlayerObjectPlugin {
    fn build(&self, app: &mut App) {
        app.configure_sets(
            PreUpdate,
            PreSpawnedPlayerObjectSet::Spawn.in_set(PredictionSet::SpawnPrediction),
        );
        app.configure_sets(
            PostUpdate,
            PreSpawnedPlayerObjectSet::CleanUp.in_set(PredictionSet::All),
        );
        app.configure_sets(
            FixedPostUpdate,
            // we run the prespawn hash at FixedUpdate AND PostUpdate (to handle entities spawned during Update)
            // TODO: entities spawned during update might have a tick that is off by 1 or more...
            //  account for this when setting the hash?
            // NOTE: we need to call this before SpawnHistory otherwise the history would affect the hash.
            // TODO: find a way to exclude predicted history from the hash
            InternalReplicationSet::<ClientMarker>::SetPreSpawnedHash
                .in_set(PredictionSet::All)
                .before(PredictionSet::SpawnHistory),
        );

        app.add_systems(
            PreUpdate,
            // we first try to see if the entity was a PreSpawnedPlayerObject
            // if we couldn't match it then the component gets removed and then should we try the normal spawn-prediction flow
            // TODO: or should we just consider that there was an error, and not go through the normal prediction flow?
            (Self::match_with_received_server_entity, apply_deferred)
                .chain()
                .in_set(PreSpawnedPlayerObjectSet::Spawn),
        );
        app.add_systems(
            FixedPostUpdate,
            // compute hashes for all pre-spawned player objects
            Self::compute_prespawn_hash
                .in_set(InternalReplicationSet::<ClientMarker>::SetPreSpawnedHash),
        );

        app.add_systems(
            PostUpdate,
            (
                Self::pre_spawned_player_object_cleanup.in_set(PreSpawnedPlayerObjectSet::CleanUp),
                // TODO: right now we only support pre-spawning during FixedUpdate::Main because we need the exact
                //  tick to compute the hash
                // compute hashes for all pre-spawned player objects
                // Self::compute_prespawn_hash
                //     .in_set(InternalReplicationSet::<ClientMarker>::SetPreSpawnedHash),
            ),
        );
    }
}

impl PreSpawnedPlayerObjectPlugin {
    /// Compute the hash of the prespawned entity by hashing the type of all its components along with the tick at which it was created
    pub(crate) fn compute_prespawn_hash(
        // ignore replicated entities, we only want to iterate through entities spawned on the client
        // directly
        // we need a param-set because of https://github.com/bevyengine/bevy/issues/7255
        // (entity-mut conflicts with resources)
        mut set: ParamSet<(
            Query<
                (EntityRef, &PreSpawnedPlayerObject),
                (
                    Without<Replicated>,
                    Without<Confirmed>,
                    Added<PreSpawnedPlayerObject>,
                ),
            >,
            ResMut<PredictionManager>,
        )>,
        component_registry: Res<ComponentRegistry>,
        tick_manager: Res<TickManager>,
        rollback: Res<Rollback>,
        components: &Components,
    ) {
        let mut prediction_manager = std::mem::take(&mut *set.p1());
        // get the rollback tick if the pre-spawned entity is being recreated during rollback!
        let tick = tick_manager.tick_or_rollback_tick(rollback.as_ref());
        for (entity_ref, prespawn) in set.p0().iter() {
            let entity = entity_ref.id();
            let hash = prespawn.hash.map_or_else(
                || {
                    let new_hash = compute_default_hash(
                        &component_registry,
                        components,
                        entity_ref.archetype(),
                        tick,
                    );
                    // No need to set the value on the component here, we only need the value in the resource!
                    // prespawn.hash = Some(new_hash);
                    debug!(?entity, ?tick, hash = ?new_hash, "computed spawn hash for entity");
                    new_hash
                },
                |hash| {
                    trace!(
                        ?entity,
                        ?tick,
                        ?hash,
                        "the hash has already been computed for the entity!"
                    );
                    hash
                },
            );

            // check if we can match with an existing server entity that was received
            // before the client entity was spawned
            // this could happen if we are predicting remote players:
            // - client 1 presses input and spawns a prespawned-object
            // - the pre-spawned object AND the input are replicated to player 2
            // - player 2 receives BOTH the replicated object and the input, and spawns a duplicate object

            // TODO: what to do in multiple entities share the same hash?
            //  just match a random one of them? or should the user have a more precise hash?
            prediction_manager
                .prespawn_hash_to_entities
                .entry(hash)
                .or_default()
                .push(entity);
            // add a timer on the entity so that it gets despawned if the interpolation tick
            // reaches it without matching with any server entity
            prediction_manager.prespawn_tick_to_hash.push(tick, hash);
            // predicted_entities.push(entity);
        }

        *set.p1() = prediction_manager;

        // NOTE: originally I wanted to remove PreSpawnedPlayerObject here because I wanted to call `compute_hash`
        // at PostUpdate, which would run twice (at the end of FixedUpdate and at PostUpdate)
        // But actually we need the component to be present so that we spawn a ComponentHistory

        // for entity in predicted_entities {
        //     info!("remove PreSpawnedPlayerObject");
        //     // we stored the relevant information in the PredictionManager resource
        //     // so we can remove the component here
        //     world.entity_mut(entity).remove::<PreSpawnedPlayerObject>();
        // }
    }

    // TODO: should we require that ShouldBePredicted is present on the entity?
    /// When we receive an entity from the server that contains the PreSpawnedPlayerObject component,
    /// that means that we already spawned it on the client.
    /// Try to match which client entity it is and take authority over it.
    pub(crate) fn match_with_received_server_entity(
        mut commands: Commands,
        connection: Res<ConnectionManager>,
        mut manager: ResMut<PredictionManager>,
        // TODO: replace with Query<&PreSpawnedPlayerObject, Added<Replicating>> ?
        mut events: EventReader<ComponentInsertEvent<PreSpawnedPlayerObject>>,
        query: Query<&PreSpawnedPlayerObject>,
    ) {
        for event in events.read() {
            let confirmed_entity = event.entity();
            // we handle the PreSpawnedPlayerObject hash in this system and don't need it afterwards
            commands
                .entity(confirmed_entity)
                .remove::<PreSpawnedPlayerObject>();
            let server_prespawn = query.get(confirmed_entity).unwrap();

            let Some(server_hash) = server_prespawn.hash else {
                debug!("Received a PreSpawnedPlayerObject entity from the server without a hash");
                continue;
            };
            let Some(mut client_entity_list) =
                manager.prespawn_hash_to_entities.remove(&server_hash)
            else {
                debug!(?server_hash, "Received a PreSpawnedPlayerObject entity from the server with a hash that does not match any client entity");
                // remove the PreSpawnedPlayerObject so that the entity can be normal-predicted
                commands
                    .entity(confirmed_entity)
                    .remove::<PreSpawnedPlayerObject>();
                continue;
            };

            // if there are multiple entities, we will use the first one
            let client_entity = client_entity_list.pop().unwrap();
            debug!("found a client pre-spawned entity corresponding to server pre-spawned entity! Spawning a Predicted entity for it");

            // we found the corresponding client entity!
            // 1.a if the client_entity exists, remove the PreSpawnedPlayerObject component from the client entity
            //  and add a Predicted component to it
            let predicted_entity =
                if let Some(mut entity_commands) = commands.get_entity(client_entity) {
                    debug!("re-using existing entity");
                    entity_commands
                        .remove::<PreSpawnedPlayerObject>()
                        .insert(Predicted {
                            confirmed_entity: Some(confirmed_entity),
                        });
                    client_entity
                } else {
                    debug!("spawning new entity");
                    // 1.b if the client_entity does not exist, re-create it (because server has authority)
                    commands
                        .spawn(Predicted {
                            confirmed_entity: Some(confirmed_entity),
                        })
                        .id()
                };

            // 2. assign Confirmed to the server entity's counterpart, and remove PreSpawnedPlayerObject
            // get the confirmed tick for the entity
            // if we don't have it, something has gone very wrong
            let confirmed_tick = connection
                .replication_receiver
                .get_confirmed_tick(confirmed_entity)
                .unwrap();
            commands
                .entity(confirmed_entity)
                .insert(Confirmed {
                    predicted: Some(predicted_entity),
                    interpolated: None,
                    tick: confirmed_tick,
                })
                // remove ShouldBePredicted so that we don't spawn another Predicted entity
                .remove::<(PreSpawnedPlayerObject, ShouldBePredicted)>();
            debug!(
                "Added/Spawned the Predicted entity: {:?} for the confirmed entity: {:?}",
                predicted_entity, confirmed_entity
            );

            // 3. re-add the remaining entities in the map
            if !client_entity_list.is_empty() {
                manager
                    .prespawn_hash_to_entities
                    .insert(server_hash, client_entity_list);
            }
        }
    }

    /// Cleanup the client prespawned entities for which we couldn't find a mapped server entity
    pub(crate) fn pre_spawned_player_object_cleanup(
        mut commands: Commands,
        tick_manager: Res<TickManager>,
        connection: Res<ConnectionManager>,
        mut manager: ResMut<PredictionManager>,
    ) {
        let tick = tick_manager.tick();
        // TODO: why is interpolation tick not good enough and we need to use an earlier tick?
        // TODO: for some reason at interpolation_tick we often haven't received the update from the server yet!
        //  use a tick that it's even more in the past
        let interpolation_tick = connection.sync_manager.interpolation_tick(&tick_manager);
        trace!(
            ?tick,
            ?interpolation_tick,
            "cleaning up prespawned player objects"
        );
        // NOTE: cannot assert because of tick_wrap tests
        // assert!(
        //     tick >= interpolation_tick,
        //     "tick {:?} should be greater than interpolation_tick {:?}",
        //     tick,
        //     interpolation_tick
        // );
        let tick_diff = (tick - interpolation_tick).saturating_mul(2) as u16;
        let past_tick = tick - tick_diff;
        // remove all the prespawned entities that have not been matched with a server entity
        for (_, hash) in manager.prespawn_tick_to_hash.drain_until(&past_tick) {
            manager
                .prespawn_hash_to_entities
                .remove(&hash)
                .iter()
                .flatten()
                .for_each(|entity| {
                    if let Some(entity_commands) = commands.get_entity(*entity) {
                        trace!(
                            ?tick,
                            ?entity,
                            "Cleaning up prespawned player object up to past tick: {:?}",
                            past_tick
                        );
                        entity_commands.despawn_recursive();
                    }
                });
        }
    }
}

#[derive(
    Component, Serialize, Deserialize, Default, Debug, Copy, Clone, PartialEq, Eq, Reflect,
)]
pub struct PreSpawnedPlayerObject {
    /// The hash that will identify the spawned entity
    /// By default, if the hash is not set, it will be generated from the entity's archetype (list of components) and spawn tick
    /// Otherwise you can manually set it to a value that will be the same on both the client and server
    pub hash: Option<u64>,
    //
    // pub conflict_resolution: ConflictResolution,
}

// pub enum ClientNoMatchHandling {
//     /// If we don't get any server-entity that matches this prespawned player object, then we despawn it on the client
//     /// Once we are sure that we won't get any more server updates for that entity
//     /// (i.e. once interpolation_tick is reached)
//     Despawn,
//
//     /// Even if we don't get any server-entity that matches this prespawned player object, we don't bother despawning it
//     /// and we just leave it as is
//     Allow,
// }
//
// pub enum ServerNoMatchHandling {
//     /// If the server sends an entity that doesn't match any existing client prespawned player object, we consider that the server
//     /// entity is still valid and we spawn a Predicted entity for it.
//     ForcePrediction,
// }

// pub enum ConflictResolution {
//     /// If we don't get any server-entity that matches this prespawned player object, then we despawn it on the client
//     /// Once we are sure that we won't get any more server updates for that entity
//     /// (i.e. once interpolation_tick is reached)
//     DespawnClient,
//     /// If the server sends us an entity that doesn't match any client prespawned player object, we consider that the entity
//     /// should still be predicted normally.
//     AllowDuplicate,
// }

// TODO: maybe provide a prediction_spawn command instead of running the `compute_hash` in both FixedUpdate and PostUpdate?

// /// This command must be used to spawn predicted entities
// /// - It will insert the
// /// - If the entity is confirmed, we despawn both the predicted and confirmed entities
// pub struct PredictionSpawnCommand {
//     entity: Entity,
//     _marker: PhantomData,
// }
//
// impl Command for PredictionSpawnCommand {
//     fn apply(self, world: &mut World) {
//         todo!()
//     }
// }
//
// pub trait PredictionSpawnCommandsExt {
//     fn prediction_spawn(&mut self);
//
// }
// impl PredictionSpawnCommandsExt for EntityCommands<'_, '_, '_> {
//     fn prediction_spawn(&mut self, pre) {
//         let entity = self.id();
//         self.commands().add(PredictionDespawnCommand {
//             entity,
//             _marker: PhantomData::,
//         })
//     }
// }

// At the end of Update, maintain a HashMap from hash -> entity for the client-side pre-spawned entities
// when we get a server entity with PreSpawned

// pub enum PredictedMode {
//     /// The entity is spawned on the server and then replicated to the client, which will spawn a Confirmed and a Predicted entity
//     FromServer,
//     /// The entity is spawned on the client, which will send a message to the server to tell it to spawn an entity.

//     /// The client can predict-spawn an entity, and it expects the server to also spawn the same entity when it receives
//     /// the information about the first entity.
//     /// Then the server replicates back its spawned entity to the client, and grabs authority over the entity.
//     /// All inputs that act on the entity after its spawned will be sent to the server.
//     PreSpawnedUserControlled {
//         /// the client entity that was pre-spawned and will be sent to the server
//         client_entity: Option<Entity>,
//         /// this is set by the server to know which client did the pre-prediction (in case the client is running
//         /// prediction for other client's entities as well)
//         client_id: Option<ClientId>,
//     },
//     /// The entity is created on both the client (in the predicted-timeline) and server side, preferably with the same system.
//     /// (for example a bullet that is shot by the player)
//     /// When the server replicates the bullet to the client, it finds the corresponding client prespawned entity and takes authority over it.
//     PreSpawnedPlayerObject {
//         /// Hash that will identify the entity
//         hash: u64,
//     },
// }

#[cfg(test)]
mod tests {
    use crate::client::prediction::predicted_history::{ComponentState, PredictionHistory};
    use bevy::prelude::Entity;
    use hashbrown::HashMap;

    use crate::client::prediction::resource::PredictionManager;

    use crate::prelude::*;
    use crate::tests::protocol::*;
    use crate::tests::stepper::{BevyStepper, Step};
    use crate::utils::ready_buffer::ItemWithReadyKey;

    #[test]
    fn test_compute_hash() {
        let mut stepper = BevyStepper::default();

        // check default compute hash, with multiple entities sharing the same tick
        stepper
            .client_app
            .world_mut()
            .spawn((Component1(1.0), PreSpawnedPlayerObject::default()));
        stepper
            .client_app
            .world_mut()
            .spawn((Component1(1.0), PreSpawnedPlayerObject::default()));
        stepper.frame_step();

        let current_tick = stepper.client_app.world().resource::<TickManager>().tick();
        let prediction_manager = stepper.client_app.world().resource::<PredictionManager>();
        let expected_hash: u64 = 14837968436853353711;
        assert_eq!(
            prediction_manager.prespawn_hash_to_entities,
            HashMap::from_iter(vec![(
                expected_hash,
                vec![Entity::from_raw(0), Entity::from_raw(1)]
            )])
        );
        assert_eq!(
            prediction_manager.prespawn_tick_to_hash.heap.peek(),
            Some(&ItemWithReadyKey {
                key: current_tick,
                item: expected_hash,
            })
        );

        // check that a PredictionHistory got added to the entity
        assert_eq!(
            stepper
                .client_app
                .world()
                .entity(Entity::from_raw(0))
                .get::<PredictionHistory<Component1>>()
                .unwrap()
                .buffer
                .heap
                .peek(),
            Some(&ItemWithReadyKey {
                key: current_tick,
                item: ComponentState::Updated(Component1(1.0)),
            })
        );
    }
}
