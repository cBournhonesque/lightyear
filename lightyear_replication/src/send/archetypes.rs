//! Keep track of the archetypes that should be replicated
use crate::hierarchy::ReplicateLike;
use crate::registry::ComponentKind;
use crate::registry::registry::ComponentRegistry;
use crate::send::components::{Replicate, Replicating};
use alloc::vec::Vec;
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
///
// NOTE: we keep the generic so that we can have both resources in the same world in
// host-server mode
#[derive(Resource)]
pub(crate) struct ReplicatedArchetypes {
    /// ID of the component identifying if the archetype is used for Replication: `Replicate`
    replication_component_id: ComponentId,
    /// ID of the [`Replicating`] component, which indicates that the entity is being replicated.
    /// If this component is not present, we pause all replication (inserts/updates/spawns)
    replicating_component_id: ComponentId,
    /// ID of the [`ReplicateLike`] component. If present, we will replicate with the same parameters as the
    /// entity stored in `ReplicateLike`
    replicate_like_component_id: ComponentId,
    /// Highest processed archetype ID.
    generation: ArchetypeGeneration,

    /// Archetypes marked as replicated.
    pub(crate) archetypes: HashMap<ArchetypeId, Vec<ReplicatedComponent>>,
}

impl FromWorld for ReplicatedArchetypes {
    fn from_world(world: &mut World) -> Self {
        Self {
            replication_component_id: world.register_component::<Replicate>(),
            replicating_component_id: world.register_component::<Replicating>(),
            replicate_like_component_id: world.register_component::<ReplicateLike>(),
            generation: ArchetypeGeneration::initial(),
            archetypes: HashMap::default(),
        }
    }
}

#[derive(Debug)]
pub(crate) struct ReplicatedComponent {
    pub(crate) id: ComponentId,
    pub(crate) kind: ComponentKind,
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
        for archetype in archetypes[old_generation..].iter().filter(|archetype| {
            archetype.contains(self.replicate_like_component_id)
                || (archetype.contains(self.replication_component_id)
                    && archetype.contains(self.replicating_component_id))
        }) {
            let mut replicated_archetype = Vec::new();

            // add all components of the archetype that are present in the ComponentRegistry
            archetype.components().for_each(|component| {
                let info = unsafe { components.get_info(component).unwrap_unchecked() };
                // if the component has a type_id (i.e. is a rust type)
                if let Some(kind) = info.type_id().map(ComponentKind) {
                    // the component is not registered for replication in the ComponentProtocol
                    let Some(replication_metadata) = registry.replication_map.get(&kind) else {
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
                        has_overrides: has_replication_overrides,
                    });
                }
            });
            self.archetypes.insert(archetype.id(), replicated_archetype);
        }
    }
}
