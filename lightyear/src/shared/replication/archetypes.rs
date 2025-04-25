//! Keep track of the archetypes that should be replicated
use core::mem;

use crate::client::replication::send::ReplicateToServer;
use crate::prelude::{ChannelDirection, ComponentRegistry, ReplicateLike, Replicating};
use crate::protocol::component::ComponentKind;
use crate::server::replication::send::ReplicateToClient;
use crate::shared::replication::authority::HasAuthority;
use bevy::ecs::archetype::Archetypes;
use bevy::ecs::component::Components;
use bevy::platform::collections::HashMap;
use bevy::{
    ecs::{
        archetype::{ArchetypeGeneration, ArchetypeId},
        component::ComponentId,
    },
    prelude::*,
};
use tracing::trace;

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
    pub(crate) archetypes: HashMap<ArchetypeId, Vec<ReplicatedComponent>>,
    marker: core::marker::PhantomData<C>,
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
            archetypes: HashMap::default(),
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
            archetypes: HashMap::default(),
            marker: Default::default(),
        }
    }
}

pub(crate) struct ReplicatedComponent {
    pub(crate) id: ComponentId,
    pub(crate) kind: ComponentKind,
}

impl<C: Component> ReplicatedArchetypes<C> {
    /// Update the list of archetypes that should be replicated.
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
                    && archetype.contains(self.replicating_component_id)
                    // on the client, we only replicate if we have authority
                    // (on the server, we need to replicate to other clients even if we don't have authority)
                    && self
                        .has_authority_component_id
                        .map_or(true, |id| archetype.contains(id)))
        }) {
            let mut replicated_archetype = Vec::new();
            // add all components of the archetype that are present in the ComponentRegistry, and:
            // - ignore component if the component is disabled
            // - check if delta-compression is enabled
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
                    // ignore the components that are not registered for replication in this direction
                    if !(self.send_direction)(replication_metadata.direction) {
                        trace!(
                            "not including {:?} because it doesn't replicate in this direction",
                            info.name()
                        );
                        return;
                    }
                    trace!("including {:?} in replicated components", info.name());
                    replicated_archetype.push(ReplicatedComponent {
                        id: component,
                        kind,
                    });
                }
            });
            self.archetypes.insert(archetype.id(), replicated_archetype);
        }
    }
}
