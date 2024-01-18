//! Handles spawning entities that are predicted

use crate::client::components::Confirmed;
use crate::client::connection::ConnectionManager;
use crate::client::events::ComponentInsertEvent;
use crate::client::prediction::despawn::PredictionDespawnCommand;
use crate::client::prediction::resource::PredictionManager;
use crate::client::prediction::Predicted;
use crate::netcode::ClientId;
use crate::prelude::{ShouldBePredicted, Tick, TickManager};
use crate::protocol::Protocol;
use crate::shared::replication::components::{DespawnTracker, Replicate};
use bevy::ecs::archetype::Archetype;
use bevy::ecs::component::Components;
use bevy::ecs::system::{Command, EntityCommands};
use bevy::prelude::{
    Added, Commands, Component, DespawnRecursiveExt, DetectChanges, Entity, EntityRef,
    EntityWorldMut, EventReader, Mut, ParamSet, Query, Ref, Res, ResMut, With, Without, World,
};
use lightyear_macros::MessageInternal;
use serde::{Deserialize, Serialize};
use std::any::{Any, TypeId};
use std::hash::{BuildHasher, Hash, Hasher};
use std::marker::PhantomData;
use tracing::{error, info, trace};

#[derive(
    MessageInternal, Component, Serialize, Deserialize, Default, Debug, Copy, Clone, PartialEq, Eq,
)]
pub struct PreSpawnedPlayerObject {
    /// The hash that will identify the spawned entity
    /// By default, if the hash is not set, it will be generated from the entity's archetype (list of components) and spawn tick
    /// Otherwise you can manually set it to a value that will be the same on both the client and server
    pub hash: Option<u64>,
    //
    // pub conflict_resolution: ConflictResolution,
}

#[derive(Component)]
pub struct ClientPreSpawnedPlayerObject {
    despawn_tick: Tick,
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
// pub struct PredictionSpawnCommand<P: Protocol> {
//     entity: Entity,
//     _marker: PhantomData<P>,
// }
//
// impl<P: Protocol> Command for PredictionSpawnCommand<P> {
//     fn apply(self, world: &mut World) {
//         todo!()
//     }
// }
//
// pub trait PredictionSpawnCommandsExt {
//     fn prediction_spawn<P: Protocol>(&mut self);
//
// }
// impl PredictionSpawnCommandsExt for EntityCommands<'_, '_, '_> {
//     fn prediction_spawn<P: Protocol>(&mut self, pre) {
//         let entity = self.id();
//         self.commands().add(PredictionDespawnCommand {
//             entity,
//             _marker: PhantomData::<P>,
//         })
//     }
// }

/// Compute the hash of the spawned entity by hashing the type of all its components along with the tick at which it was created
pub(crate) fn compute_hash<P: Protocol>(world: &mut World) {
    // let (tick, interpolation_tick) =
    //     world.resource_scope(|world: &mut World, connection: Mut<ConnectionManager<P>>| {
    //         let tick_manager = world.resource::<TickManager>();
    //         let interpolation_tick = connection.sync_manager.interpolation_tick(&tick_manager);
    //         (tick_manager.tick(), interpolation_tick)
    //     });
    let tick = world.resource::<TickManager>().tick();
    world.resource_scope(|world: &mut World, mut manager: Mut<PredictionManager>| {
        let components = world.components();

        // ignore confirmed entities just in case we somehow didn't remove their hash during PreUpdate
        let mut pre_spawned_query =
            world.query_filtered::<(EntityRef, Ref<PreSpawnedPlayerObject>), Without<Confirmed>>();
        let mut predicted_entities = vec![];
        for (entity_ref, prespawn) in pre_spawned_query.iter(world) {
            // we only care about newly-added PreSpawnedPlayerObject components
            if !prespawn.is_added() {
                continue;
            }
            let entity = entity_ref.id();
            let hash = prespawn.hash.map_or_else(
                || {
                    // TODO: try EntityHasher instead since we only hash the 64 lower bits of TypeId
                    // TODO: should I create the hasher once outside?
                    let mut hasher =
                        bevy::utils::RandomState::with_seeds(1, 2, 3, 4).build_hasher();
                    // TODO: the default hasher doesn't seem to be deterministic across processes
                    // let mut hasher = bevy::utils::AHasher::default();

                    // TODO: this only works currently for entities that are spawned during Update!
                    //  if we want the tick to be valid, compute_hash should also be run at the end of FixedUpdate::Main
                    //  so that we have the exact spawn tick! Solutions:
                    //  run compute_hash in post-update as well
                    // we include the spawn tick in the hash
                    info!("including tick {:?} for hash", tick);
                    tick.hash(&mut hasher);

                    // NOTE: we cannot call hash() multiple times because the components in the archetype
                    //  might get iterated in any order!
                    //  Instead we will get the sorted list of types to hash first, sorted by type_id
                    let mut types_to_hash = entity_ref
                        .archetype()
                        .components()
                        .filter_map(|component_id| {
                            if let Some(type_id) =
                                world.components().get_info(component_id).unwrap().type_id()
                            {
                                // ignore some book-keeping components
                                if type_id != TypeId::of::<Replicate<P>>()
                                    && type_id != TypeId::of::<ShouldBePredicted>()
                                    && type_id != TypeId::of::<DespawnTracker>()
                                {
                                    return Some(type_id);
                                }
                            }
                            None
                        })
                        .collect::<Vec<_>>();
                    types_to_hash.sort();
                    types_to_hash.into_iter().for_each(|type_id| {
                        trace!(?type_id, "using type id for hash");
                        type_id.hash(&mut hasher)
                    });

                    // No need to set the value on the component here, we only need the value in the resource!
                    // prespawn.hash = Some(hasher.finish());

                    let new_hash = hasher.finish();
                    info!(?entity, ?tick, hash = ?new_hash, "computed spawn hash for entity");
                    new_hash
                },
                |hash| {
                    info!(
                        ?entity,
                        ?tick,
                        ?hash,
                        "the hash has already been computed for the entity!"
                    );
                    hash
                },
            );

            // TODO: what to do in multiple entities share the same hash?
            //  just match a random one of them? or should the user have a more precise hash?
            manager
                .prespawn_hash_to_entities
                .entry(hash)
                .or_default()
                .push(entity);
            // add a timer on the entity so that it gets despawned if the interpolation tick
            // reaches it without matching with any server entity
            manager.prespawn_tick_to_hash.add_item(tick, hash);
            predicted_entities.push(entity);
        }

        for entity in predicted_entities {
            info!("remove PreSpawnedPlayerObject");
            // we stored the relevant information in the PredictionManager resource
            // so we can remove the component here
            world.entity_mut(entity).remove::<PreSpawnedPlayerObject>();
        }
    });
}

/// Cleanup the client prespawned entities for which we couldn't find a mapped server entity
pub(crate) fn pre_spawned_player_object_cleanup<P: Protocol>(
    mut commands: Commands,
    tick_manager: Res<TickManager>,
    connection: Res<ConnectionManager<P>>,
    mut manager: ResMut<PredictionManager>,
) {
    let interpolation_tick = connection.sync_manager.interpolation_tick(&tick_manager);
    // remove all the prespawned entities that have not been matched with a server entity
    for (_, hash) in manager
        .prespawn_tick_to_hash
        .drain_until(&interpolation_tick)
    {
        manager
            .prespawn_hash_to_entities
            .remove(&hash)
            .iter()
            .flatten()
            .for_each(|entity| {
                if let Some(entity_commands) = commands.get_entity(*entity) {
                    info!(?entity, "Cleaning up prespawned player object");
                    entity_commands.despawn_recursive();
                }
            });
    }
}

// At the end of Update, maintain a HashMap from hash -> entity for the client-side pre-spawned entities
// when we get a server entity with PreSpawned

// TODO: should we require that ShouldBePredicted is present on the entity?
/// When we receive an entity from the server that contains the PreSpawnedPlayerObject component,
/// that means that we already spawned it on the client.
/// Try to match which client entity it is and take authority over it.
pub(crate) fn spawn_pre_spawned_player_object<P: Protocol>(
    mut commands: Commands,
    connection: Res<ConnectionManager<P>>,
    mut manager: ResMut<PredictionManager>,
    mut events: EventReader<ComponentInsertEvent<PreSpawnedPlayerObject>>,
    query: Query<&PreSpawnedPlayerObject>,
) {
    for event in events.read() {
        let confirmed_entity = event.entity();
        // // we handle the PreSpawnedPlayerObject hash in this system and don't need it afterwards
        // commands
        //     .entity(confirmed_entity)
        //     .remove::<PreSpawnedPlayerObject>();
        let server_prespawn = query.get(confirmed_entity).unwrap();

        let Some(server_hash) = server_prespawn.hash else {
            error!("Received a PreSpawnedPlayerObject entity from the server without a hash");
            continue;
        };
        let Some(mut client_entity_list) = manager.prespawn_hash_to_entities.remove(&server_hash)
        else {
            error!(?server_hash, "Received a PreSpawnedPlayerObject entity from the server with a hash that does not match any client entity");
            // remove the PreSpawnedPlayerObject so that the entity can be normal-predicted
            commands
                .entity(confirmed_entity)
                .remove::<PreSpawnedPlayerObject>();
            continue;
        };

        // if there are multiple entities, we will use the first one
        let client_entity = client_entity_list.pop().unwrap();
        info!("found a client pre-spawned entity corresponding to server pre-spawned entity! Spawning a Predicted entity for it");

        // we found the corresponding client entity!
        // 1.a if the client_entity exists, remove the PreSpawnedPlayerObject component from the client entity
        //  and add a Predicted component to it
        let predicted_entity = if let Some(mut entity_commands) = commands.get_entity(client_entity)
        {
            info!("re-using existing entity");
            entity_commands
                .remove::<PreSpawnedPlayerObject>()
                .insert(Predicted {
                    confirmed_entity: Some(confirmed_entity),
                });
            client_entity
        } else {
            info!("spawning new entity");
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
            .remove::<PreSpawnedPlayerObject>();
        info!(
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
