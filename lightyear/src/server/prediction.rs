//! Handles logic related to prespawning entities

use std::any::TypeId;
use std::hash::{Hash, Hasher};

use crate::client::replication::send::ReplicateToServer;
use bevy::ecs::component::Components;
use bevy::prelude::*;

use crate::prelude::server::ControlledBy;
use crate::prelude::{
    ComponentRegistry, ParentSync, PreSpawnedPlayerObject, ReplicateHierarchy, Replicated,
    Replicating, ReplicationTarget, ShouldBePredicted, TargetEntity, TickManager, VisibilityMode,
};
use crate::protocol::component::ComponentKind;
use crate::server::replication::send::SyncTarget;
use crate::server::visibility::immediate::ReplicateVisibility;
use crate::shared::replication::components::DespawnTracker;

/// Compute the hash of the spawned entity by hashing the NetId of all its components along with the tick at which it was created
/// 1. Client spawns an entity and adds the PreSpawnedPlayerObject component
/// 2. Client will compute the hash of the entity and store it internally
/// 3. Server (later) spawns the entity, computes the hash and replicates the PreSpawnedPlayerObject component
/// 4. When the client receives the PreSpawnedPlayerObject component, it will compare the hash with the one it computed
pub(crate) fn compute_hash(
    // we need a param-set because of https://github.com/bevyengine/bevy/issues/7255
    // (entity-mut conflicts with resources)
    mut set: ParamSet<(
        Query<EntityMut, Added<PreSpawnedPlayerObject>>,
        Res<ComponentRegistry>,
        Res<TickManager>,
    )>,
    components: &Components,
) {
    let tick = set.p2().tick();
    let net_id_map = set.p1().kind_map.kind_map.clone();

    // get the list of entities that need to have a new hash computed, along with the hash
    for mut entity_mut in set.p0().iter_mut() {
        let entity = entity_mut.id();
        // the hash has already been computed by the user
        if entity_mut
            .get::<PreSpawnedPlayerObject>()
            .unwrap()
            .hash
            .is_some()
        {
            trace!("Hash for pre-spawned player object was already computed!");
            continue;
        }
        // let mut hasher = bevy::utils::RandomState::with_seeds(1, 2, 3, 4).build_hasher();
        let mut hasher = seahash::SeaHasher::new();
        // let mut hasher = xxhash_rust::xxh3::Xxh3Builder::new()
        //     .with_seed(1)
        //     .build_hasher();
        // TODO: the default hasher doesn't seem to be deterministic across processes
        // let mut hasher = bevy::utils::AHasher::default();

        // TODO: figure out how to hash the spawn tick
        tick.hash(&mut hasher);

        // NOTE: we cannot call hash() multiple times because the components in the archetype
        //  might get iterated in any order!
        //  Instead we will get the sorted list of types to hash first, sorted by net_id
        let mut kinds_to_hash = entity_mut
            .archetype()
            .components()
            .filter_map(|component_id| {
                if let Some(type_id) = components.get_info(component_id).unwrap().type_id() {
                    // ignore some book-keeping components
                    if type_id != TypeId::of::<VisibilityMode>()
                        && type_id != TypeId::of::<ReplicationTarget>()
                        && type_id != TypeId::of::<SyncTarget>()
                        && type_id != TypeId::of::<ControlledBy>()
                        && type_id != TypeId::of::<Replicating>()
                        && type_id != TypeId::of::<Replicated>()
                        && type_id != TypeId::of::<ReplicateToServer>()
                        && type_id != TypeId::of::<ReplicateVisibility>()
                        && type_id != TypeId::of::<VisibilityMode>()
                        && type_id != TypeId::of::<TargetEntity>()
                        && type_id != TypeId::of::<ReplicateHierarchy>()
                        && type_id != TypeId::of::<PreSpawnedPlayerObject>()
                        && type_id != TypeId::of::<ShouldBePredicted>()
                        && type_id != TypeId::of::<DespawnTracker>()
                        && type_id != TypeId::of::<ParentSync>()
                    {
                        return net_id_map.get(&ComponentKind::from(type_id)).copied();
                    }
                }
                None
            })
            .collect::<Vec<_>>();
        kinds_to_hash.sort();
        kinds_to_hash.into_iter().for_each(|kind| {
            trace!(?kind, "using kind for hash");
            kind.hash(&mut hasher)
        });

        let hash = hasher.finish();
        debug!(?entity, ?tick, ?hash, "computed spawn hash for entity");
        let mut prespawn = entity_mut.get_mut::<PreSpawnedPlayerObject>().unwrap();
        prespawn.hash = Some(hash);
    }
}
