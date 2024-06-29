//! Keep track of the archetypes that should be replicated
use std::mem;

use crate::prelude::{ChannelDirection, ComponentRegistry, Replicating};
use crate::protocol::component::ComponentKind;
use crate::shared::plugin::Identity;
use bevy::ecs::archetype::ArchetypeEntity;
use bevy::ecs::component::{ComponentTicks, StorageType};
use bevy::ecs::storage::{SparseSets, Table};
use bevy::ptr::Ptr;
use bevy::{
    ecs::{
        archetype::{ArchetypeGeneration, ArchetypeId},
        component::ComponentId,
    },
    prelude::*,
};

/// Cached information about all replicated archetypes.
///
/// The generic component is the component that is used to identify if the archetype is used for Replication.
/// This is the [`ReplicateToServer`](crate::client::replication::send::ReplicateToServer) or [`ReplicationTarget`](crate::prelude::ReplicationTarget) component.
/// (not the [`Replicating`], which just indicates if we are in the process of replicating.
pub(crate) struct ReplicatedArchetypes<C: Component> {
    /// ID of the component identifying if the archetype is used for Replication.
    /// This is the [`ReplicateToServer`](crate::client::replication::send::ReplicateToServer) or [`ReplicationTarget`](crate::prelude::ReplicationTarget) component.
    /// (not the [`Replicating`], which just indicates if we are in the process of replicating.
    replication_component_id: ComponentId,
    /// ID of the [`Replicating`] component, which indicates that the entity is being replicated.
    /// If this component is not present, we pause all replication (inserts/updates/spawns)
    replicating_component_id: ComponentId,
    /// Highest processed archetype ID.
    generation: ArchetypeGeneration,

    /// Archetypes marked as replicated.
    pub(crate) archetypes: Vec<ReplicatedArchetype>,
    marker: std::marker::PhantomData<C>,
}

fn should_replicate(world: &World, direction: ChannelDirection) -> bool {
    let identity = Identity::get_from_world(world);
    match direction {
        ChannelDirection::ClientToServer => identity.is_client(),
        ChannelDirection::ServerToClient => identity.is_server(),
        ChannelDirection::Bidirectional => true,
    }
}

impl<C: Component> FromWorld for ReplicatedArchetypes<C> {
    fn from_world(world: &mut World) -> Self {
        Self {
            replication_component_id: world.init_component::<C>(),
            replicating_component_id: world.init_component::<Replicating>(),
            generation: ArchetypeGeneration::initial(),
            archetypes: Vec::new(),
            marker: std::marker::PhantomData,
        }
    }
}

/// An archetype that should have some components replicated
pub(crate) struct ReplicatedArchetype {
    pub(crate) id: ArchetypeId,
    pub(crate) components: Vec<ReplicatedComponent>,
}

pub(crate) struct ReplicatedComponent {
    pub(crate) delta_compression: Option<ComponentId>,
    pub(crate) replicate_once: bool,
    pub(crate) override_target: Option<ComponentId>,
    pub(crate) id: ComponentId,
    pub(crate) kind: ComponentKind,
    pub(crate) storage_type: StorageType,
}

/// Get the component data as a [`Ptr`] and its change ticks
///
/// # Safety
///
/// Component should be present in the Table or SparseSet
pub(crate) unsafe fn get_erased_component<'w>(
    table: &'w Table,
    sparse_sets: &'w SparseSets,
    entity: &ArchetypeEntity,
    storage_type: StorageType,
    component_id: ComponentId,
) -> (Ptr<'w>, ComponentTicks) {
    match storage_type {
        StorageType::Table => {
            let column = table.get_column(component_id).unwrap_unchecked();
            let component = column.get_data_unchecked(entity.table_row());
            let ticks = column.get_ticks_unchecked(entity.table_row());

            (component, ticks)
        }
        StorageType::SparseSet => {
            let sparse_set = sparse_sets.get(component_id).unwrap_unchecked();
            let component = sparse_set.get(entity.id()).unwrap_unchecked();
            let ticks = sparse_set.get_ticks(entity.id()).unwrap_unchecked();

            (component, ticks)
        }
    }
}

impl<C: Component> ReplicatedArchetypes<C> {
    /// Update the list of archetypes that should be replicated.
    pub(crate) fn update(&mut self, world: &World, registry: &ComponentRegistry) {
        let old_generation = mem::replace(&mut self.generation, world.archetypes().generation());

        // iterate through the newly added archetypes
        for archetype in world.archetypes()[old_generation..]
            .iter()
            .filter(|archetype| {
                archetype.contains(self.replication_component_id)
                    && archetype.contains(self.replicating_component_id)
            })
        {
            let mut replicated_archetype = ReplicatedArchetype {
                id: archetype.id(),
                components: Vec::new(),
            };
            // TODO: pause inserts/updates if Replicating is not present on the entity!
            // add all components of the archetype that are present in the ComponentRegistry, and:
            // - ignore component if the component is disabled (DisabledComponent<C>) is present
            // - check if delta-compression is enabled
            archetype.components().for_each(|component| {
                let info = unsafe { world.components().get_info(component).unwrap_unchecked() };
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
                    trace!("including {:?} in replicated components", info.name());

                    // check per component metadata
                    let disabled = archetype
                        .components()
                        .any(|c| c == replication_metadata.disabled_id);
                    // we do not replicate the component
                    if disabled {
                        return;
                    }
                    // TODO: should we store the components in a hashmap for faster lookup?
                    let delta_compression = archetype
                        .components()
                        .any(|c| c == replication_metadata.delta_compression_id)
                        .then_some(replication_metadata.delta_compression_id);
                    let replicate_once = archetype
                        .components()
                        .any(|c| c == replication_metadata.replicate_once_id);
                    let override_target = archetype
                        .components()
                        .any(|c| c == replication_metadata.override_target_id)
                        .then_some(replication_metadata.override_target_id);

                    let disabled = archetype
                        .components()
                        .any(|c| c == replication_metadata.disabled_id);
                    // SAFETY: component ID obtained from this archetype.
                    let storage_type =
                        unsafe { archetype.get_storage_type(component).unwrap_unchecked() };
                    replicated_archetype.components.push(ReplicatedComponent {
                        delta_compression,
                        replicate_once,
                        override_target,
                        id: component,
                        kind,
                        storage_type,
                    });
                }
            });
            self.archetypes.push(replicated_archetype);
        }
    }
}
