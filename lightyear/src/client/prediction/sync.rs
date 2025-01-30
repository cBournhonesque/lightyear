//! Systems that will sync components from the Confirmed entity to the Predicted entity

use bevy::ecs::system::SystemChangeTick;
use bevy::prelude::{Local, Res, World};
use crate::client::components::Confirmed;
use crate::client::confirmed::ConfirmedArchetypes;
use crate::client::prediction::archetypes::PredictedArchetypes;
use crate::client::prediction::Predicted;
use crate::prelude::{ComponentRegistry, TickManager};

fn sync(
    world: &mut World,
    tick_manager: Res<TickManager>,
    component_registry: Res<ComponentRegistry>,
    mut confirmed_archetypes: Local<ConfirmedArchetypes>,
    mut predicted_archetypes: Local<PredictedArchetypes>,
) {
    // update the archetype caches
    confirmed_archetypes.update(world, &component_registry);
    predicted_archetypes.update(world, &component_registry);

    // TODO: it's a waste because a portion of these will be Confirmed but only have an interpolated entity, not confirmed! maybe have a ZST ConfirmedPredicted to make that a Predicted component exists?
    // go through all the archetypes that have the Confirmed component
    for confirmed_archetype in confirmed_archetypes.archetypes.iter() {
        // SAFETY: update() makes sure that we have a valid archetype
        let archetype = unsafe {
            world
                .archetypes()
                .get(confirmed_archetype.id)
                .unwrap_unchecked()
        };
        let table = unsafe {
            world
                .storages()
                .tables
                .get(archetype.table_id())
                .unwrap_unchecked()
        };

        // go through all entities on the confirmed archetype
        for entity in archetype.entities() {
            let confirmed = entity.id();
            let Some(predicted) = world.get::<Confirmed>(confirmed).unwrap().predicted else {
                continue;
            };
            let [confirmed_world_mut, predicted_world_mut] = world.entity_mut([confirmed, predicted]);

            // TODO: is it better to do it here or via observers? Probably via
            //  observers so that we don't have to iterate through all the components
            //  to check if any were added?
            //  maybe the better option is to buffer the write via observers in a temp buffer,
            //  and then write them all at once on the entity?

            // sync components if Confirmed was added on a pre-existing entity (for example if client 1 spawns an entity and transfers authority to server)

            // sync components added on the confirmed entity, and add a history if necessary

            // NOTE: this must be done via observers!
            // sync components removed on confirmed entity

            // TODO: call component_registry.batch_sync which will run insert_by_ids, insert ComponentHistory, etc.
            for component in confirmed_archetype.predicted_components {

            }

        }
    }

    // go through all the archetypes that have the Predicted components
    for predicted_archetype in predicted_archetypes.archetypes.iter() {
        // SAFETY: update() makes sure that we have a valid archetype
        let archetype = unsafe {
            world
                .archetypes()
                .get(predicted_archetype.id)
                .unwrap_unchecked()
        };
        let table = unsafe {
            world
                .storages()
                .tables
                .get(archetype.table_id())
                .unwrap_unchecked()
        };

        // go through all entities on the predicted archetype
        for entity in archetype.entities() {
            let predicted = entity.id();
            let Some(confirmed) = world.get::<Predicted>(predicted).unwrap().confirmed_entity else {
                continue;
            };
            let [predicted_world_mut, confirmed_world_mut] = world.entity_mut([predicted, confirmed]);

            // for components added on the predicted entity, we might need to spawn a PredictionHistory

            // add history for components added on PreSpawned entities
        }
    }
}