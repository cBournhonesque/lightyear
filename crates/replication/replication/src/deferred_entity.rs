//! Deferred structural entity commands.

use alloc::{boxed::Box, vec::Vec};
use bevy_ecs::{
    change_detection::DetectChangesMut,
    component::Component,
    entity::{Entity, EntityHashMap},
    system::Commands,
};
use bevy_replicon::shared::replication::deferred_entity::{DeferredEntity, EntityScratch};
use core::any::TypeId;

type ApplyDeferredEntityCommand =
    Box<dyn for<'w> FnOnce(&mut DeferredEntity<'w>) + Send + Sync + 'static>;

#[derive(Default)]
struct DeferredEntityMutations {
    commands: Vec<ApplyDeferredEntityCommand>,
    removals: Vec<TypeId>,
    mark_all_changed: bool,
}

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
    entities: EntityHashMap<DeferredEntityMutations>,
}

impl DeferredEntityCommands {
    /// Queues insertion of one component on `entity`.
    pub fn insert<C: Component>(&mut self, entity: Entity, component: C) {
        self.entities
            .entry(entity)
            .or_default()
            .commands
            .push(Box::new(move |entity| {
                entity.insert(component);
            }));
    }

    /// Queues removal of one component from `entity`.
    ///
    /// Repeated removals of the same component from one entity are coalesced because Replicon's
    /// dynamic removal bundle requires unique component IDs.
    pub fn remove<C: Component>(&mut self, entity: Entity) {
        let mutations = self.entities.entry(entity).or_default();
        let component = TypeId::of::<C>();
        if mutations.removals.contains(&component) {
            return;
        }
        mutations.removals.push(component);
        mutations.commands.push(Box::new(|entity| {
            entity.remove::<C>();
        }));
    }

    /// Marks every existing mutable component on `entity` as changed.
    ///
    /// This is useful when removing a disabling component: changed-only systems did not see the
    /// entity while it was disabled, so they need to revisit its existing components once it is
    /// enabled. Components buffered for insertion are already marked changed by their insertion.
    pub fn mark_all_changed(&mut self, entity: Entity) {
        self.entities.entry(entity).or_default().mark_all_changed = true;
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
                for mutation in mutations.commands {
                    mutation(&mut deferred);
                }
                deferred.flush();

                if mutations.mark_all_changed {
                    let Ok(mut entity_mut) = world.get_entity_mut(entity) else {
                        continue;
                    };
                    // Copy one ID at a time so no temporary component-ID collection is needed.
                    // Marking a component as changed cannot alter the entity's archetype.
                    let component_count = entity_mut.archetype().components().len();
                    for index in 0..component_count {
                        let component_id = entity_mut.archetype().components()[index];
                        if let Ok(mut component) = entity_mut.get_mut_by_id(component_id) {
                            component.set_changed();
                        }
                    }
                }
            }
        });
    }
}
