//! Keep track of the archetypes that should be replicated
use crate::hierarchy::{ReplicateLike, ReplicateLikeChildren};
use crate::prelude::{NetworkVisibility, ReplicationGroup};
use crate::registry::ComponentKind;
use crate::registry::registry::ComponentRegistry;
use crate::send::components::{Replicate, Replicating};
use alloc::vec::Vec;
use bevy_ecs::component::StorageType;
use bevy_ecs::{
    archetype::{ArchetypeGeneration, ArchetypeId, Archetypes},
    component::{ComponentId, Components},
    resource::Resource,
    world::{FromWorld, World},
};
use bevy_platform::collections::HashMap;
use core::mem;
use tracing::trace;

/// Cached information about the replicated archetypes for a given sender.
/// This is used to iterate faster over the components that need to be replicated for a given entity.
#[derive(Resource)]
pub(crate) struct ReplicatedArchetypes {
    /// ID of the component identifying if the archetype is used for Replication: `Replicate`
    replication: ComponentId,
    /// ID of the [`Replicating`] component, which indicates that the entity is being replicated.
    /// If this component is not present, we pause all replication (inserts/updates/spawns)
    replicating: ComponentId,
    /// ID of the [`ReplicateLike`] component. If present, we will replicate with the same parameters as the
    /// entity stored in `ReplicateLike`
    replicate_like: ComponentId,
    /// ID of the [`ReplicateLikeChildren`] component.
    replicate_like_children: ComponentId,
    replication_group: ComponentId,
    network_visibility: ComponentId,
    /// Highest processed archetype ID.
    generation: ArchetypeGeneration,

    /// Archetypes for entities that have [`ReplicateLike`] marked as replicated.
    pub(crate) child_archetypes: HashMap<ArchetypeId, ReplicatedArchetype>,
    /// Root archetypes for replication: entities that have [`Replicate`] and no [`ReplicateLike`]
    pub(crate) root_archetypes: Vec<(ArchetypeId, ReplicatedArchetype)>,
    /// Archetypes that have [`ReplicationGroup`], we want all entities in the group to be replicated together
    pub(crate) group_archetypes: Vec<ArchetypeId>,
}

impl FromWorld for ReplicatedArchetypes {
    fn from_world(world: &mut World) -> Self {
        Self {
            replication: world.register_component::<Replicate>(),
            replicating: world.register_component::<Replicating>(),
            replicate_like: world.register_component::<ReplicateLike>(),
            replicate_like_children: world.register_component::<ReplicateLikeChildren>(),
            replication_group: world.register_component::<ReplicationGroup>(),
            network_visibility: world.register_component::<NetworkVisibility>(),
            generation: ArchetypeGeneration::initial(),
            child_archetypes: HashMap::default(),
            root_archetypes: Vec::default(),
            group_archetypes: Vec::default(),
        }
    }
}

pub(crate) struct ReplicatedArchetype {
    pub(crate) components: Vec<ReplicatedComponent>,
    pub(crate) has_replicate_like_children: bool,
    pub(crate) has_network_visibility: bool,
}

#[derive(Debug)]
pub(crate) struct ReplicatedComponent {
    pub(crate) id: ComponentId,
    pub(crate) kind: ComponentKind,
    pub(crate) storage_type: StorageType,
    pub(crate) delta_compression: bool,
    pub(crate) has_overrides: bool,
}

impl ReplicatedArchetypes {
    /// Update the list of entities/components that should be replicated for this sender
    pub(crate) fn update(
        &mut self,
        archetypes: &Archetypes,
        components: &Components,
        registry: &ComponentRegistry,
    ) {
        let old_generation = mem::replace(&mut self.generation, archetypes.generation());

        // iterate through the newly added archetypes
        for archetype in archetypes[old_generation..].iter() {
            if archetype.contains(self.replication_group) {
                self.group_archetypes.push(archetype.id());
                return;
            }
            let has_replicate_like = archetype.contains(self.replicate_like);
            let is_root = !has_replicate_like
                && archetype.contains(self.replication)
                && archetype.contains(self.replicating);
            if has_replicate_like || is_root {
                let mut replicated_archetype = Vec::new();

                // add all components of the archetype that are present in the ComponentRegistry
                archetype.iter_components().for_each(|component| {
                    let info = unsafe { components.get_info(component).unwrap_unchecked() };
                    // if the component has a type_id (i.e. is a rust type)
                    if let Some(kind) = info.type_id().map(ComponentKind) {
                        // the component is not registered for replication in the ComponentProtocol
                        let Some(replication_metadata) = registry
                            .component_metadata_map
                            .get(&kind)
                            .and_then(|m| m.replication.as_ref())
                        else {
                            trace!(
                                "not including {:?} because it is not registered for replication",
                                info.name()
                            );
                            return;
                        };

                        let has_replication_overrides =
                            archetype.contains(replication_metadata.overrides_component_id);

                        // ignore components that are disabled by default and don't have overrides
                        if replication_metadata.config.disable && !has_replication_overrides {
                            trace!(
                                "not including {:?} because it is disabled by default",
                                info.name()
                            );
                            return;
                        }
                        trace!("including {:?} in replicated components", info.name());
                        replicated_archetype.push(ReplicatedComponent {
                            id: component,
                            kind,
                            // SAFETY: we know the component exists in the archetype
                            storage_type: unsafe {
                                archetype.get_storage_type(component).unwrap_unchecked()
                            },
                            delta_compression: replication_metadata.config.delta_compression,
                            has_overrides: has_replication_overrides,
                        });
                    }
                });
                if is_root {
                    self.root_archetypes.push((
                        archetype.id(),
                        ReplicatedArchetype {
                            components: replicated_archetype,
                            has_replicate_like_children: archetype
                                .contains(self.replicate_like_children),
                            has_network_visibility: archetype.contains(self.network_visibility),
                        },
                    ));
                } else {
                    self.child_archetypes.insert(
                        archetype.id(),
                        ReplicatedArchetype {
                            components: replicated_archetype,
                            has_replicate_like_children: false,
                            has_network_visibility: archetype.contains(self.network_visibility),
                        },
                    );
                }
            }
        }
    }
}
