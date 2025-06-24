use crate::Predicted;
use crate::registry::PredictionRegistry;
use alloc::vec::Vec;
use bevy_ecs::{
    archetype::{ArchetypeGeneration, ArchetypeId, Archetypes},
    component::{ComponentId, Components},
    resource::Resource,
    world::{FromWorld, World},
};
use bevy_platform::collections::HashMap;
use lightyear_replication::registry::ComponentKind;
use lightyear_replication::registry::registry::ComponentRegistry;
use tracing::trace;

/// Cached list of archetypes that are predicted.
///
/// These archetypes have at least one component that needs to be synced from the Predicted entity
/// to the Confirmed entity.
///
/// We cache them to quickly iterate through all the components of an archetype that should be predicted
#[derive(Resource)]
pub(crate) struct PredictedArchetypes {
    /// Highest processed archetype ID.
    generation: ArchetypeGeneration,
    predicted_component_id: ComponentId,
    /// Cached archetypes
    pub(crate) archetypes: HashMap<ArchetypeId, Vec<PredictedComponent>>,
}

/// A component that is predicted
pub(crate) struct PredictedComponent {
    pub(crate) id: ComponentId,
    pub(crate) kind: ComponentKind,
}

impl FromWorld for PredictedArchetypes {
    fn from_world(world: &mut World) -> Self {
        Self {
            generation: ArchetypeGeneration::initial(),
            predicted_component_id: world.register_component::<Predicted>(),
            archetypes: HashMap::default(),
        }
    }
}

impl PredictedArchetypes {
    /// Update the list of predicted archetypes by going through all newly-added archetypes
    pub(crate) fn update(
        &mut self,
        archetypes: &Archetypes,
        components: &Components,
        prediction_registry: &PredictionRegistry,
    ) {
        let old_generation = core::mem::replace(&mut self.generation, archetypes.generation());

        // iterate through the newly added archetypes
        for archetype in archetypes[old_generation..]
            .iter()
            .filter(|archetype| archetype.contains(self.predicted_component_id))
        {
            let mut predicted_archetype = Vec::new();
            // add all components from the registry that are predicted
            archetype.components().for_each(|component| {
                let info = unsafe { components.get_info(component).unwrap_unchecked() };
                // if the component has a type_id (i.e. is a rust type)
                if let Some(kind) = info.type_id().map(ComponentKind) {
                    // the component is not registered for prediction in the ComponentProtocol
                    let Some(prediction_metadata) = prediction_registry.prediction_map.get(&kind) else {
                        trace!(
                            "not including {:?} in the cached predicted archetype because it is not registered for prediction",
                            info.name()
                        );
                        return;
                    };
                    let storage_type =
                        unsafe { archetype.get_storage_type(component).unwrap_unchecked() };
                    predicted_archetype.push(PredictedComponent {
                        id: component,
                        kind,
                    });
                }
            });
            self.archetypes.insert(archetype.id(), predicted_archetype);
        }
    }
}
