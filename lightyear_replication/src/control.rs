use bevy::ecs::component::HookContext;
use bevy::ecs::entity::EntityIndexSet;
use bevy::ecs::relationship::{Relationship, RelationshipHookMode, RelationshipSourceCollection};
use bevy::ecs::world::DeferredWorld;
use bevy::prelude::*;
use lightyear_connection::client::{Disconnected, PeerMetadata};
use lightyear_core::id::PeerId;
use lightyear_core::prelude::LocalTimeline;
use serde::{Deserialize, Serialize};

/// Marker component on the receiver side to indicate that the entity is under the
/// control of the local peer
#[derive(Component, Clone, PartialEq, Debug, Reflect, Serialize, Deserialize)]
pub struct Controlled;

/// Component on the sender side that lists the entities controlled by the local peer
#[derive(Component, Clone, PartialEq, Debug, Reflect)]
#[relationship_target(relationship = OwnedBy)]
#[reflect(Component)]
pub struct Owned(Vec<Entity>);

// TODO: ideally the user can specify a PeerId as sender, and we would find the corresponding entity.
//  we have a map from PeerId to the corresponding entity?

#[derive(Component, Clone, Copy, PartialEq, Debug, Reflect)]
#[reflect(Component)]
#[component(on_insert = OwnedBy::on_insert)]
#[component(on_replace = OwnedBy::on_replace)]
pub struct OwnedBy {
    /// Which peer controls this entity?
    pub owner: PeerId,
    /// What happens to the entity if the controlling client disconnects?
    pub lifetime: Lifetime,
}

impl Relationship for OwnedBy {
    type RelationshipTarget = Owned;

    fn get(&self) -> Entity {
        panic!("this should not be used");
    }

    fn from(entity: Entity) -> Self {
        panic!("this should not be used");
    }
}

impl OwnedBy {
    pub(crate) fn handle_disconnection(
        trigger: Trigger<OnAdd, Disconnected>,
        mut commands: Commands,
        owned: Query<&Owned>,
        owned_by: Query<&OwnedBy>,
    ) {
        if let Ok(owned) = owned.get(trigger.target()) {
            trace!("Despawning Owned entities because client disconnected");
            for entity in owned.collection() {
                if let Ok(owned_by) = owned_by.get(*entity) {
                    match owned_by.lifetime {
                        Lifetime::SessionBased => {
                            trace!(
                                "Despawning entity {entity:?} controlled by disconnected client {:?}",
                                trigger.target()
                            );
                            commands.entity(*entity).try_despawn();
                        }
                        Lifetime::Persistent => {}
                    }
                }
            }
        }
    }

    /// The `on_insert` component hook that maintains the [`OwnedBy`] / [`Owned`] connection.
    fn on_insert(
        mut world: DeferredWorld,
        HookContext {
            entity,
            caller,
            relationship_hook_mode,
            ..
        }: HookContext,
    ) {
        match relationship_hook_mode {
            RelationshipHookMode::Run => {}
            RelationshipHookMode::Skip => return,
            RelationshipHookMode::RunIfNotLinked => {
                return;
            }
        }
        let owned_by = world.entity(entity).get::<Self>().unwrap();
        let Some(&target_entity) = world
            .resource::<PeerMetadata>()
            .mapping
            .get(&owned_by.owner)
        else {
            warn!(
                "The owner {:?} does not exist. Removing `OwnedBy`",
                owned_by.owner
            );
            world.commands().entity(entity).try_remove::<Self>();
            return;
        };
        if target_entity == entity {
            warn!(
                "{}The {}({target_entity:?}) relationship on entity {entity:?} points to itself. The invalid {} relationship has been removed.",
                caller
                    .map(|location| format!("{location}: "))
                    .unwrap_or_default(),
                core::any::type_name::<Self>(),
                core::any::type_name::<Self>()
            );
            world.commands().entity(entity).try_remove::<Self>();
            return;
        }
        if let Ok(mut target_entity_mut) = world.get_entity_mut(target_entity) {
            if let Some(mut relationship_target) = target_entity_mut.get_mut::<Owned>() {
                relationship_target.collection_mut_risky().add(entity);
            } else {
                let mut target = <Owned as RelationshipTarget>::with_capacity(1);
                target.collection_mut_risky().add(entity);
                world.commands().entity(target_entity).insert(target);
            }
        } else {
            warn!(
                "{}The {}({target_entity:?}) relationship on entity {entity:?} relates to an entity that does not exist. The invalid {} relationship has been removed.",
                caller
                    .map(|location| format!("{location}: "))
                    .unwrap_or_default(),
                core::any::type_name::<Self>(),
                core::any::type_name::<Self>()
            );
            world.commands().entity(entity).try_remove::<Self>();
        }
    }

    /// The `on_replace` component hook that maintains the [`Relationship`] / [`RelationshipTarget`] connection.
    // note: think of this as "on_drop"
    fn on_replace(
        mut world: DeferredWorld,
        HookContext {
            entity,
            relationship_hook_mode,
            ..
        }: HookContext,
    ) {
        match relationship_hook_mode {
            RelationshipHookMode::Run => {}
            RelationshipHookMode::Skip => return,
            RelationshipHookMode::RunIfNotLinked => {
                if Owned::LINKED_SPAWN {
                    return;
                }
            }
        }
        let owner = world.entity(entity).get::<Self>().unwrap().owner;
        let Some(&target_entity) = world.resource::<PeerMetadata>().mapping.get(&owner) else {
            if let Ok(mut entity_mut) = world.commands().get_entity(entity) {
                trace!("The owner {:?} does not exist. Removing `OwnedBy`", owner);
                entity_mut.try_remove::<Self>();
            }
            return;
        };
        if let Ok(mut target_entity_mut) = world.get_entity_mut(target_entity) {
            if let Some(mut relationship_target) = target_entity_mut.get_mut::<Owned>() {
                RelationshipSourceCollection::remove(
                    relationship_target.collection_mut_risky(),
                    entity,
                );
                if relationship_target.len() == 0 {
                    if let Ok(mut entity) = world.commands().get_entity(target_entity) {
                        // this "remove" operation must check emptiness because in the event that an identical
                        // relationship is inserted on top, this despawn would result in the removal of that identical
                        // relationship ... not what we want!
                        entity.queue(|mut entity: EntityWorldMut| {
                            if entity
                                .get::<Owned>()
                                .is_some_and(RelationshipTarget::is_empty)
                            {
                                entity.remove::<Owned>();
                            }
                        });
                    }
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Reflect)]
pub enum Lifetime {
    #[default]
    /// When the client that controls the entity disconnects, the entity is despawned
    SessionBased,
    /// The entity is not despawned even if the controlling client disconnects
    Persistent,
}
