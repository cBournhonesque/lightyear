//! Deferred structural entity commands.

use alloc::{boxed::Box, vec::Vec};
use bevy_ecs::{
    component::Component,
    entity::{Entity, EntityHashMap},
    system::Commands,
};
use bevy_replicon::shared::replication::deferred_entity::{DeferredEntity, EntityScratch};

type ApplyDeferredEntityCommand =
    Box<dyn for<'w> FnOnce(&mut DeferredEntity<'w>) + Send + Sync + 'static>;

/// Batched structural changes for multiple entities.
///
/// This is a thin [`Commands`] wrapper around Replicon's [`DeferredEntity`].
/// Use it when a system needs to defer live component insertions/removals until
/// after an entity query has finished. Changes are grouped by entity and
/// flushed through one Replicon [`DeferredEntity`] per entity.
///
/// ```
/// use bevy_ecs::prelude::*;
/// use lightyear_replication::deferred_entity::DeferredEntityCommands;
///
/// #[derive(Component)]
/// struct Position(f32);
///
/// #[derive(Component)]
/// struct NeedsPosition;
///
/// fn add_positions(mut commands: Commands, entities: Query<Entity, With<NeedsPosition>>) {
///     let mut deferred = DeferredEntityCommands::default();
///     for entity in &entities {
///         deferred.insert(entity, Position(1.0));
///     }
///     deferred.apply(&mut commands);
/// }
/// ```
#[derive(Default)]
pub struct DeferredEntityCommands {
    entities: EntityHashMap<Vec<ApplyDeferredEntityCommand>>,
}

impl DeferredEntityCommands {
    /// Queues insertion of one component on `entity`.
    pub fn insert<C: Component>(&mut self, entity: Entity, component: C) {
        self.entities
            .entry(entity)
            .or_default()
            .push(Box::new(move |entity| {
                entity.insert(component);
            }));
    }

    /// Queues removal of one component from `entity`.
    pub fn remove<C: Component>(&mut self, entity: Entity) {
        self.entities
            .entry(entity)
            .or_default()
            .push(Box::new(|entity| {
                entity.remove::<C>();
            }));
    }

    /// Queues a Bevy command that applies all deferred mutations.
    pub fn apply(self, commands: &mut Commands) {
        if self.entities.is_empty() {
            return;
        }
        commands.queue(move |world: &mut bevy_ecs::world::World| {
            let mut scratch = EntityScratch::default();
            for (entity, mutations) in self.entities {
                let Ok(entity_mut) = world.get_entity_mut(entity) else {
                    continue;
                };
                let mut deferred = DeferredEntity::new(entity_mut, &mut scratch);
                for mutation in mutations {
                    mutation(&mut deferred);
                }
                deferred.flush();
            }
        });
    }
}
