//! Keep track of the archetypes that should be replicated
use std::mem;

use crate::client::replication::send::ReplicateToServer;
use crate::prelude::{ChannelDirection, ComponentRegistry, ReplicateLike, Replicating};
use crate::protocol::component::ComponentKind;
use crate::server::replication::send::ReplicateToClient;
use crate::shared::replication::authority::HasAuthority;
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
/// This is the [`ReplicateToServer`] or [`ReplicateToClient`] component.
/// (not the [`Replicating`], which just indicates if we are in the process of replicating.
// NOTE: we keep the generic so that we can have both resources in the same world in
// host-server mode
#[derive(Resource)]
pub(crate) struct ReplicatedArchetypes<C: Component> {
    /// Function that returns true if the direction is compatible with sending from this peer
    send_direction: SendDirectionFn,
    /// ID of the component identifying if the archetype is used for Replication.
    /// This is the [`ReplicateToServer`] or [`ReplicateToClient`] component.
    /// (not the [`Replicating`], which just indicates if we are in the process of replicating.
    replication_component_id: ComponentId,
    /// ID of the [`Replicating`] component, which indicates that the entity is being replicated.
    /// If this component is not present, we pause all replication (inserts/updates/spawns)
    replicating_component_id: ComponentId,
    /// ID of the [`ReplicateLike`] component. If present, we will replicate with the same parameters as the
    /// entity stored in `ReplicateLike`
    replicate_like_component_id: ComponentId,
    /// ID of the [`HasAuthority`] component, which indicates that the current peer has authority over the entity.
    /// On the client, we only send replication updates if we have authority.
    /// On the server, we still send replication updates even if we don't have authority, because
    /// we need to relay the changes to other clients.
    has_authority_component_id: Option<ComponentId>,
    /// Highest processed archetype ID.
    generation: ArchetypeGeneration,

    /// Archetypes marked as replicated.
    pub(crate) archetypes: Vec<ReplicatedArchetype>,
    marker: std::marker::PhantomData<C>,
}

pub type SendDirectionFn = fn(ChannelDirection) -> bool;

fn send_to_server(direction: ChannelDirection) -> bool {
    matches!(
        direction,
        ChannelDirection::Bidirectional | ChannelDirection::ClientToServer
    )
}

fn send_to_client(direction: ChannelDirection) -> bool {
    matches!(
        direction,
        ChannelDirection::Bidirectional | ChannelDirection::ServerToClient
    )
}

pub(crate) type ClientReplicatedArchetypes = ReplicatedArchetypes<ReplicateToServer>;
pub(crate) type ServerReplicatedArchetypes = ReplicatedArchetypes<ReplicateToClient>;

impl FromWorld for ClientReplicatedArchetypes {
    fn from_world(world: &mut World) -> Self {
        Self::client(world)
    }
}

impl FromWorld for ServerReplicatedArchetypes {
    fn from_world(world: &mut World) -> Self {
        Self::server(world)
    }
}

impl<C: Component> ReplicatedArchetypes<C> {
    pub(crate) fn client(world: &mut World) -> Self {
        Self {
            send_direction: send_to_server,
            replication_component_id: world.register_component::<ReplicateToServer>(),
            replicating_component_id: world.register_component::<Replicating>(),
            replicate_like_component_id: world.register_component::<ReplicateLike>(),
            has_authority_component_id: Some(world.register_component::<HasAuthority>()),
            generation: ArchetypeGeneration::initial(),
            archetypes: Vec::new(),
            marker: Default::default(),
        }
    }

    pub(crate) fn server(world: &mut World) -> Self {
        Self {
            send_direction: send_to_client,
            replication_component_id: world.register_component::<ReplicateToClient>(),
            replicating_component_id: world.register_component::<Replicating>(),
            replicate_like_component_id: world.register_component::<ReplicateLike>(),
            has_authority_component_id: None,
            generation: ArchetypeGeneration::initial(),
            archetypes: Vec::new(),
            marker: Default::default(),
        }
    }
}

/// An archetype that should have some components replicated
pub(crate) struct ReplicatedArchetype {
    pub(crate) id: ArchetypeId,
    pub(crate) components: Vec<ReplicatedComponent>,
}

pub(crate) struct ReplicatedComponent {
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
            let component = table
                .get_component(component_id, entity.table_row())
                .unwrap_unchecked();
            let ticks = table
                .get_ticks_unchecked(component_id, entity.table_row())
                .unwrap_unchecked();
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
                archetype.contains(self.replicate_like_component_id)
                    || (archetype.contains(self.replication_component_id)
                    && archetype.contains(self.replicating_component_id)
                    // on the client, we only replicate if we have authority
                    // (on the server, we need to replicate to other clients even if we don't have authority)
                    && self
                        .has_authority_component_id
                        .map_or(true, |id| archetype.contains(id)))
            })
        {
            let mut replicated_archetype = ReplicatedArchetype {
                id: archetype.id(),
                components: Vec::new(),
            };
            // add all components of the archetype that are present in the ComponentRegistry, and:
            // - ignore component if the component is disabled
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
                    // ignore the components that are not registered for replication in this direction
                    if !(self.send_direction)(replication_metadata.direction) {
                        trace!(
                            "not including {:?} because it doesn't replicate in this direction",
                            info.name()
                        );
                        return;
                    }
                    trace!("including {:?} in replicated components", info.name());

                    // SAFETY: component ID obtained from this archetype.
                    let storage_type =
                        unsafe { archetype.get_storage_type(component).unwrap_unchecked() };
                    replicated_archetype.components.push(ReplicatedComponent {
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
