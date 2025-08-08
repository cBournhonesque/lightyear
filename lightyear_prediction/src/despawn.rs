use crate::Predicted;
use crate::manager::PredictionResource;
use crate::prespawn::PreSpawned;
use bevy_ecs::error::ignore;
use bevy_ecs::{
    component::Component,
    entity::Entity,
    error::Result,
    observer::Trigger,
    reflect::ReflectComponent,
    system::{Command, Commands, EntityCommands, Query},
    world::{EntityWorldMut, OnRemove, World},
};
use bevy_reflect::Reflect;
use lightyear_connection::host::HostClient;
use lightyear_replication::prelude::{Confirmed, ShouldBePredicted};
use tracing::{error, info};

/// This command must be used to despawn Predicted entities.
/// The reason is that we might want to not completely despawn the entity in case it gets 'restored' during a rollback.
/// (i.e. we do a rollback and we realize the entity should not have been despawned)
/// Instead we will Disable the entity so that it stops showing up.
///
/// The general flow is:
/// - we run predicted_despawn on the predicted entity
/// - `PredictedDespawnDisable` is added on the entity. We use our own custom marker instead of Disable in case users want to genuinely just
///   disable a Predicted entity.
/// - We can stop updating its PredictionHistory, or only update it with empty values (None)
/// - if the Confirmed entity is also despawned in the next few ticks, then the Predicted entity also gets despawned
/// - we still do rollback checks using the Confirmed updates against the `PredictedDespawn` entity! If there is a rollback,
///   we can remove the Disabled marker on all predicted entities, restore all their components to the Confirmed value, and then
///   re-run the last few-ticks (which might re-Disable the entity)
pub struct PredictionDespawnCommand {
    entity: Entity,
}

#[derive(Component, PartialEq, Debug, Reflect)]
#[reflect(Component)]
pub struct PredictionDisable;

impl Command for PredictionDespawnCommand {
    fn apply(self, world: &mut World) {
        // if we are the server (or host-client), there is no rollback so we can despawn the entity immediately
        if world
            .get_resource::<PredictionResource>()
            .is_none_or(|r| world.entity(r.link_entity).contains::<HostClient>())
            && let Ok(e) = world.get_entity_mut(self.entity)
        {
            e.despawn();
        };

        if let Ok(mut entity) = world.get_entity_mut(self.entity) {
            if entity.get::<Predicted>().is_some()
                || entity.get::<ShouldBePredicted>().is_some()
                // see https://github.com/cBournhonesque/lightyear/issues/818
                || entity.get::<PreSpawned>().is_some()
            {
                // if this is a predicted entity, do not despawn the entity immediately but instead
                // add a PredictionDisable component to it to mark it as disabled until the confirmed
                // entity catches up to it
                info!("inserting prediction disable marker");
                entity.insert(PredictionDisable);
            } else if entity.get::<Confirmed>().is_some() {
                // TODO: actually we should never despawn directly on the client a Confirmed entity
                //  it should only get despawned when replicating!
                entity.despawn();
            } else {
                error!("This command should only be called for predicted entities!");
            }
        }
    }
}

pub trait PredictionDespawnCommandsExt {
    fn prediction_despawn(&mut self);
}

impl PredictionDespawnCommandsExt for EntityCommands<'_> {
    fn prediction_despawn(&mut self) {
        let entity = self.id();
        self.queue_handled(
            move |entity_mut: EntityWorldMut| {
                let world = entity_mut.world();
                if world
                    .get_resource::<PredictionResource>()
                    .is_some_and(|r| !world.entity(r.link_entity).contains::<HostClient>())
                {
                    PredictionDespawnCommand { entity }.apply(entity_mut.into_world_mut());
                } else {
                    // if we are the server (or host server), just despawn the entity
                    entity_mut.despawn();
                }
            },
            ignore,
        );
    }
}

/// Despawn predicted entities when the confirmed entity gets despawned
pub(crate) fn despawn_confirmed(
    trigger: Trigger<OnRemove, Confirmed>,
    query: Query<&Confirmed>,
    mut commands: Commands,
) -> Result {
    if let Ok(confirmed) = query.get(trigger.target())
        && let Some(predicted) = confirmed.predicted
        && let Ok(mut entity_mut) = commands.get_entity(predicted)
    {
        entity_mut.try_despawn();
    }
    Ok(())
}
