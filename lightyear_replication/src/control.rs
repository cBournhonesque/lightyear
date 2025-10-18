use alloc::vec::Vec;
use bevy_ecs::prelude::*;
use bevy_reflect::Reflect;
use lightyear_connection::client::Disconnected;
use serde::{Deserialize, Serialize};
use tracing::trace;

/// Marker component on the receiver side to indicate that the entity is under the
/// control of the local peer that received the entity
#[derive(Component, Clone, PartialEq, Debug, Reflect, Serialize, Deserialize)]
pub struct Controlled;

/// Component on the sender side that lists the entities controlled by the remote peer
#[derive(Component, Clone, PartialEq, Debug, Reflect)]
#[relationship_target(relationship = ControlledBy)]
#[reflect(Component)]
pub struct ControlledByRemote(Vec<Entity>);

// TODO: ideally the user can specify a PeerId as sender, and we would find the corresponding entity.
//  we have a map from PeerId to the corresponding entity?

/// Sender-side component that associates the entity with a [`ReplicationSender`] 'controlling' the entity
///
/// The receiver will add a [`Controlled`] marker component upon receiving the entity.
///
/// When the link is disconnected, the sender will optionally (based on the [`Lifetime`] value)
/// despawn the entity. If you want to persist an entity on the receiver side even after the link is disconnected,
/// see [`Persistent`](super::components::Persistent)
#[derive(Component, Clone, Copy, PartialEq, Debug, Reflect)]
#[relationship(relationship_target = ControlledByRemote)]
#[reflect(Component)]
pub struct ControlledBy {
    /// Which peer controls this entity? This should be an entity with a `ReplicationSender` component
    #[relationship]
    pub owner: Entity,
    /// What happens to the entity on the sender-side if the controlling client disconnects?
    pub lifetime: Lifetime,
}

impl ControlledBy {
    pub(crate) fn handle_disconnection(
        trigger: On<Add, Disconnected>,
        mut commands: Commands,
        controlled_by_remote: Query<&ControlledByRemote>,
        controlled_by: Query<&ControlledBy>,
    ) {
        if let Ok(owned) = controlled_by_remote.get(trigger.entity) {
            trace!("Despawning Owned entities because client disconnected");
            for entity in owned.collection() {
                if let Ok(owned_by) = controlled_by.get(*entity) {
                    match owned_by.lifetime {
                        Lifetime::SessionBased => {
                            trace!(
                                "Despawning entity {entity:?} controlled by disconnected client {:?}",
                                trigger.entity
                            );
                            commands.entity(*entity).try_despawn();
                        }
                        Lifetime::Persistent => {}
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
