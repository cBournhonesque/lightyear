use bevy::ecs::entity::EntityIndexSet;
use bevy::prelude::*;
use lightyear_connection::client::Disconnected;
use lightyear_core::id::PeerId;
use serde::{Deserialize, Serialize};

/// Marker component on the receiver side to indicate that the entity is under the
/// control of the local peer
#[derive(Component, Clone, PartialEq, Debug, Reflect, Serialize, Deserialize)]
pub struct Controlled;

/// Component on the sender side that lists the entities controlled by the local peer
#[derive(Component, Clone, PartialEq, Debug, Reflect)]
#[relationship_target(relationship = OwnedBy, linked_spawn)]
#[reflect(Component)]
pub struct Owned(Vec<Entity>);

#[derive(Component, Clone, Copy, PartialEq, Debug, Reflect)]
#[relationship(relationship_target = Owned)]
#[reflect(Component)]
pub struct OwnedBy {
    #[relationship]
    /// Which peer controls this entity?
    pub sender: Entity,
    /// What happens to the entity if the controlling client disconnects?
    pub lifetime: Lifetime
}


#[derive(Clone, Copy, Debug, Default, PartialEq, Reflect)]
pub enum Lifetime {
    #[default]
    /// When the client that controls the entity disconnects, the entity is despawned
    SessionBased,
    /// The entity is not despawned even if the controlling client disconnects
    Persistent,
}


impl OwnedBy {
    pub(crate) fn handle_disconnection(
        trigger: Trigger<OnAdd, Disconnected>,
        mut commands: Commands,
        owned: Query<&Owned>,
        owned_by: Query<&OwnedBy>,
    ) {
        if let Ok(owned) = owned.get(trigger.target()) {
            for entity in owned.collection() {
                if let Ok(owned_by) = owned_by.get(*entity) {
                    match owned_by.lifetime {
                        Lifetime::SessionBased => {
                            trace!(
                                "Despawning entity {entity:?} controlled by disconnected client {:?}",
                                trigger.target()
                            );
                            commands.entity(*entity).despawn();
                        }
                        Lifetime::Persistent => {}
                    }
                }
            }
        }
    }
}