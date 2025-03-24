//! Shared logic to handle prespawning entities

use crate::prelude::{ComponentRegistry, PrePredicted, PreSpawned, ShouldBePredicted, Tick};
use crate::protocol::component::ComponentKind;
use crate::shared::replication::components::{Controlled, ShouldBeInterpolated};
use crate::shared::replication::hierarchy::ReplicateLike;
#[cfg(not(feature = "std"))]
use alloc::vec::Vec;
use bevy::ecs::archetype::Archetype;
use bevy::ecs::component::Components;
use core::any::TypeId;
use core::hash::{Hash, Hasher};
use tracing::trace;

/// Compute the default PreSpawned hash used to match server entities with prespawned client entities
pub(crate) fn compute_default_hash(
    component_registry: &ComponentRegistry,
    components: &Components,
    archetype: &Archetype,
    tick: Tick,
    salt: Option<u64>,
) -> u64 {
    // TODO: try EntityHasher instead since we only hash the 64 lower bits of TypeId
    // TODO: should I create the hasher once outside?

    // NOTE: tried
    // - bevy::utils::RandomState::with_seeds(1, 2, 3, 4).build_hasher();
    // - xxhash_rust::xxh3::Xxh3Builder::new().with_seed(1).build_hasher();
    // - bevy::utils::AHasher::default();
    // but they were not deterministic across processes
    let mut hasher = seahash::SeaHasher::new();

    // TODO: this only works currently for entities that are spawned during FixedUpdate!
    //  if we want the tick to be valid, compute_hash should also be run at the end of FixedUpdate::Main
    //  so that we have the exact spawn tick! Solutions: run compute_hash in post-update as well?
    // we include the spawn tick in the hash
    tick.hash(&mut hasher);

    // NOTE: we cannot call hash() multiple times because the components in the archetype
    //  might get iterated in any order!
    //  Instead we will get the sorted list of types to hash first, sorted by type_id
    let mut kinds_to_hash = archetype
        .components()
        .filter_map(|component_id| {
            if let Some(type_id) = components.get_info(component_id).unwrap().type_id() {
                // ignore some book-keeping components that are included in the component registry
                if type_id != TypeId::of::<PrePredicted>()
                    && type_id != TypeId::of::<PreSpawned>()
                    && type_id != TypeId::of::<ShouldBePredicted>()
                    && type_id != TypeId::of::<ShouldBeInterpolated>()
                    && type_id != TypeId::of::<Controlled>()
                    && type_id != TypeId::of::<ReplicateLike>()
                {
                    return component_registry
                        .kind_map
                        .net_id(&ComponentKind::from(type_id))
                        .copied();
                }
            }
            None
        })
        // TODO: avoid this allocation, maybe provide a preallocated vec
        .collect::<Vec<_>>();
    kinds_to_hash.sort();
    kinds_to_hash.into_iter().for_each(|kind| {
        trace!(?kind, "using kind for hash");
        kind.hash(&mut hasher)
    });

    // if a user salt is provided, hash after the sorted component list
    if let Some(salt) = salt {
        salt.hash(&mut hasher);
    }

    hasher.finish()
}
