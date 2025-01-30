use bevy::ecs::archetype::{ArchetypeGeneration, ArchetypeId};
use bevy::ecs::component::{ComponentId, StorageType};
use bevy::prelude::{FromWorld, Resource, World};
use crate::client::components::{ComponentSyncMode, Confirmed};
use crate::prelude::ComponentRegistry;
use crate::protocol::component::ComponentKind;

/// Cached list of archetypes that are confirmed.
///
/// These archetypes have at the Confirmed component.
///
/// We cache them to avoid having to run individual systems for each component
#[derive(Resource)]
pub(crate) struct ConfirmedArchetypes {
    /// Highest processed archetype ID.
    generation: ArchetypeGeneration,

    confirmed_component_id: ComponentId,
    /// Cached archetypes
    pub(crate) archetypes: Vec<ConfirmedArchetype>,
}

/// An archetype that has some predicted components
pub(crate) struct ConfirmedArchetype {
    pub(crate) id: ArchetypeId,
    pub(crate) predicted_components: Vec<SyncComponent>,
    pub(crate) interpolated_components: Vec<SyncComponent>,
}

/// A component that is synced (predicted or interpolated)
pub(crate) struct SyncComponent {
    pub(crate) id: ComponentId,
    pub(crate) kind: ComponentKind,
    pub(crate) storage_type: StorageType,
    pub(crate) sync_mode: ComponentSyncMode,
}

impl FromWorld for ConfirmedArchetypes {
    fn from_world(world: &mut World) -> Self {
        Self {
            generation: ArchetypeGeneration::initial(),
            confirmed_component_id: world.register_component::<Confirmed>(),
            archetypes: vec![],
        }
    }
}

impl ConfirmedArchetypes {

    // TODO: just need Archetypes/Components, not World
    /// Update the list of predicted archetypes by going through all newly-added archetypes
    pub(crate) fn update(&mut self, world: &World, registry: &ComponentRegistry) {
        let old_generation = core::mem::replace(&mut self.generation, world.archetypes().generation());
        // iterate through the newly added archetypes
        for archetype in world.archetypes()[old_generation..]
            .iter()
            .filter(|archetype| {
                archetype.contains(self.confirmed_component_id)
            })
        {
            let mut confirmed_archetype = ConfirmedArchetype {
                id: archetype.id(),
                predicted_components: Vec::new(),
                interpolated_components: Vec::new(),
            };
            // add all components from the registry that are predicted
            archetype.components().for_each(|component| {
                let info = unsafe { world.components().get_info(component).unwrap_unchecked() };
                // if the component has a type_id (i.e. is a rust type)
                if let Some(kind) = info.type_id().map(ComponentKind) {
                    if let Some(prediction_metadata) = registry.prediction_map.get(&kind) {
                        // the component is not registered for prediction in the ComponentProtocol
                        if prediction_metadata.sync_mode != ComponentSyncMode::None {
                            let storage_type =
                                unsafe { archetype.get_storage_type(component).unwrap_unchecked() };
                            confirmed_archetype.predicted_components.push(SyncComponent {
                                id: component,
                                kind,
                                storage_type,
                                sync_mode: prediction_metadata.sync_mode,
                            });
                        }
                    }
                    if let Some(interpolation_metadata) = registry.interpolation_map.get(&kind) {
                        // the component is not registered for interpolation in the ComponentProtocol
                        if interpolation_metadata.sync_mode != ComponentSyncMode::None {
                            let storage_type =
                                unsafe { archetype.get_storage_type(component).unwrap_unchecked() };
                            confirmed_archetype.interpolated_components.push(SyncComponent {
                                id: component,
                                kind,
                                storage_type,
                                sync_mode: interpolation_metadata.sync_mode,
                            });
                        }
                    }
                }
            });
        }
    }
}

