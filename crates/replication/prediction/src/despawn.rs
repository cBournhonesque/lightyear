use crate::Predicted;
use crate::manager::PredictionManager;
use crate::prelude::DeterministicPredicted;
use bevy_ecs::error::ignore;
use bevy_ecs::prelude::*;
use bevy_reflect::Reflect;
use lightyear_core::prelude::{LocalTimeline, Tick};
use lightyear_replication::prelude::PreSpawned;
use lightyear_sync::prelude::InputTimelineConfig;
#[allow(unused_imports)]
use tracing::{debug, error, info};
// TODO (IMPORTANT): we need to add PredictionDisable in the replication receiver systems!!!

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
pub struct PredictionDisable {
    /// Tick at which the entity was prediction-despawned.
    ///
    /// Deterministic prediction retains the entity until this tick falls outside the effective
    /// rollback window, at which point the entity can be permanently reclaimed.
    pub tick: Tick,
}

impl Command for PredictionDespawnCommand {
    type Out = ();

    fn apply(self, world: &mut World) -> Self::Out {
        // Without the application-global prediction manager there is no rollback, so the entity
        // can be despawned immediately.
        if !world.contains_resource::<PredictionManager>()
            && let Ok(e) = world.get_entity_mut(self.entity)
        {
            e.despawn();
        };

        if let Ok(mut entity) = world.get_entity_mut(self.entity) {
            if entity.get::<Predicted>().is_some()
                || entity.get::<DeterministicPredicted>().is_some()
                // see https://github.com/cBournhonesque/lightyear/issues/818
                || entity.get::<PreSpawned>().is_some()
            {
                // Do not despawn predicted entities immediately. A conventional predicted entity
                // remains disabled until confirmed replication resolves it; a deterministic-only
                // entity remains disabled until its despawn tick falls outside the rollback window.
                debug!(?self.entity, "inserting prediction disable marker");
                let disabled_at = entity.world().resource::<LocalTimeline>().tick();
                entity.insert(PredictionDisable { tick: disabled_at });
            } else {
                error!("This command should only be called for predicted entities!");
            }
        }
    }
}

/// Permanently remove deterministic entities once their despawn tick is outside the rollback
/// window.
pub(crate) fn finalize_deterministic_despawns(
    mut commands: Commands,
    timeline: Res<LocalTimeline>,
    prediction_manager: Res<PredictionManager>,
    input_config: Option<Res<InputTimelineConfig>>,
    query: Query<
        (Entity, &PredictionDisable),
        (With<DeterministicPredicted>, Allow<PredictionDisable>),
    >,
) {
    let max_rollback_ticks = input_config.as_deref().map_or(
        prediction_manager.rollback_policy.max_rollback_ticks,
        |input_config| {
            prediction_manager
                .rollback_policy
                .effective_max_rollback_ticks(input_config)
        },
    );
    for (entity, disabled) in &query {
        // Keep the entity while its disable tick can still be reached by an accepted rollback.
        // Once it is older than the same effective bound enforced by `check_rollback`, no later
        // reconciliation can restore it.
        if timeline.tick() - disabled.tick > i32::from(max_rollback_ticks) {
            commands.entity(entity).despawn();
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
                if world.contains_resource::<PredictionManager>() {
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
