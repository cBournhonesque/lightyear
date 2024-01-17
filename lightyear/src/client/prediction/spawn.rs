//! Handles spawning entities that are predicted

use crate::client::components::Confirmed;
use crate::client::connection::ConnectionManager;
use crate::client::events::ComponentInsertEvent;
use crate::client::prediction::resource::PredictionManager;
use crate::client::prediction::Predicted;
use crate::netcode::ClientId;
use crate::prelude::{ShouldBePredicted, TickManager};
use crate::protocol::Protocol;
use crate::shared::replication::components::{DespawnTracker, Replicate};
use bevy::ecs::archetype::Archetype;
use bevy::ecs::component::Components;
use bevy::prelude::{
    Added, Commands, Component, DetectChanges, Entity, EntityRef, EventReader, Mut, ParamSet,
    Query, Ref, Res, ResMut, With, Without, World,
};
use lightyear_macros::MessageInternal;
use serde::{Deserialize, Serialize};
use std::any::{Any, TypeId};
use std::hash::{BuildHasher, Hash, Hasher};
use tracing::{error, info};

#[derive(
    MessageInternal, Component, Serialize, Deserialize, Default, Debug, Copy, Clone, PartialEq, Eq,
)]
pub struct PreSpawnedPlayerObject {
    /// The hash that will identify the spawned entity
    /// By default, if the hash is not set, it will be generated from the entity's archetype (list of components) and spawn tick
    /// Otherwise you can manually set it to a value that will be the same on both the client and server
    pub hash: Option<u64>,
}

/// Compute the hash of the spawned entity by hashing the type of all its components along with the tick at which it was created
pub(crate) fn compute_hash<P: Protocol>(
    world: &mut World,
    // tick_manager: Res<TickManager>,
    // mut manager: ResMut<PredictionManager>,
    // mut set: ParamSet<(&Components, Query<(EntityRef, &mut PreSpawnedPlayerObject)>)>,
    // mut set: ParamSet<(
    //     &Components,
    //     ResMut<PredictionManager>,
    //     Query<(EntityRef, Ref<PreSpawnedPlayerObject>)>,
    // )>,
    // components: &Components,
    // pre_spawned_query: Query<(EntityRef, Ref<PreSpawnedPlayerObject>)>,
) {
    let tick = world.resource::<TickManager>().tick();
    world.resource_scope(|world: &mut World, mut manager: Mut<PredictionManager>| {
        let components = world.components();

        // ignore confirmed entities just in case we somehow didn't remove their hash during PreUpdate
        let mut pre_spawned_query =
            world.query_filtered::<(EntityRef, Ref<PreSpawnedPlayerObject>), Without<Confirmed>>();
        for (entity_ref, prespawn) in pre_spawned_query.iter(world) {
            // we only care about newly-added PreSpawnedPlayerObject components
            if !prespawn.is_added() {
                continue;
            }
            let entity = entity_ref.id();
            // the hash has already been computed by the user
            if prespawn.hash.is_some() {
                info!("Hash for pre-spawned player object was already computed!");
                manager
                    .prespawn_entities_map
                    .insert(prespawn.hash.unwrap(), entity);
                continue;
            }

            // TODO: try EntityHasher instead since we only hash the 64 lower bits of TypeId
            // TODO: should I create the hasher once outside?
            let mut hasher = bevy::utils::RandomState::with_seeds(1, 2, 3, 4).build_hasher();
            // TODO: the default hasher doesn't seem to be deterministic across processes
            // let mut hasher = bevy::utils::AHasher::default();
            // TODO: figure out how to hash the spawn tick
            // tick.hash(&mut hasher);

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
                info!(?type_id, "using type id for hash");
                type_id.hash(&mut hasher)
            });

            // No need to set the value here, we only need the value in the resource!
            // prespawn.hash = Some(hasher.finish());
            let new_hash = hasher.finish();
            info!(?entity, ?tick, hash = ?new_hash, "computed spawn hash for entity");
            // TODO: what to do in multiple entities share the same hash?
            manager.prespawn_entities_map.insert(new_hash, entity);
        }
    });

    // // it's valid to query for PreSpawnedPlayerObject because all the ones we receive from the Server
    // // have been removed by now (we run this at end of frame)
    // for (entity_ref, mut prespawn) in set.p1().iter_mut() {
    //     // the hash has already been computed by the user
    //     if prespawn.hash.is_some() {
    //         info!("Hash for pre-spawned player object was already computed!");
    //         continue;
    //     }
    //     // TODO: try EntityHasher instead since we only hash the 64 lower bits of TypeId
    //     // TODO: should I create the hasher once outside?
    //     let mut hasher = bevy::utils::AHasher::default();
    //     // TODO: figure out how to hash the spawn tick
    //     // tick.hash(&mut hasher);
    //     let entity = entity_ref.id();
    //     entity_ref
    //         .archetype()
    //         .components()
    //         .for_each(|component_id| {
    //             if let Some(type_id) = set.p0().get_info(component_id).unwrap().type_id() {
    //                 // ignore some book-keeping components
    //                 if type_id != TypeId::of::<Replicate<P>>()
    //                     || type_id != TypeId::of::<ShouldBePredicted>()
    //                 {
    //                     type_id.hash(&mut hasher)
    //                 }
    //             }
    //         });
    //     prespawn.hash = Some(hasher.finish());
    //     info!(?entity, ?tick, hash= ?prespawn.hash, "computed spawn hash for entity");
    //     manager
    //         .prespawn_entities_map
    //         .insert(prespawn.hash.unwrap(), entity);
    // }
}

// At the end of Update, maintain a HashMap from hash -> entity for the client-side pre-spawned entities
// when we get a server entity with PreSpawned

// TODO: should we require that ShouldBePredicted is present on the entity?
/// When we receive an entity from the server that contains the PreSpawnedPlayerObject component,
/// that means that we already spawned it on the client.
/// Try to match which client entity it is and take authority over it.
pub fn spawn_pre_spawned_player_object<P: Protocol>(
    mut commands: Commands,
    connection: Res<ConnectionManager<P>>,
    mut manager: ResMut<PredictionManager>,
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
            error!("Received a PreSpawnedPlayerObject entity from the server without a hash");
            continue;
        };
        let Some(client_entity) = manager.prespawn_entities_map.remove(&server_hash) else {
            error!(?server_hash, "Received a PreSpawnedPlayerObject entity from the server with a hash that does not match any client entity");
            commands
                .entity(confirmed_entity)
                .remove::<PreSpawnedPlayerObject>();
            continue;
        };

        info!("found a client entity corresponding to server entity! Spawning a Predicted entity for it");
        // we found the corresponding client entity!
        // 1.a if the client_entity exists, remove the PreSpawnedPlayerObject component from the client entity
        //  and add a Predicted component to it
        let predicted_entity = if let Some(mut entity_commands) = commands.get_entity(client_entity)
        {
            entity_commands
                .remove::<PreSpawnedPlayerObject>()
                .insert(Predicted {
                    confirmed_entity: Some(confirmed_entity),
                });
            client_entity
        } else {
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
    }
}

// pub enum PredictedMode {
//     /// The entity is spawned on the server and then replicated to the client, which will spawn a Confirmed and a Predicted entity
//     FromServer,
//     /// The entity is spawned on the client, which will send a message to the server to tell it to spawn an entity.

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
