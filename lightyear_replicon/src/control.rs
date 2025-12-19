use alloc::vec::Vec;
use bevy_app::{App, Plugin};
use bevy_derive::Deref;
use bevy_ecs::prelude::*;
use bevy_reflect::Reflect;
use bevy_replicon::prelude::{AppRuleExt, ComponentScope};
use bevy_replicon::server::visibility::client_visibility::ClientVisibility;
use bevy_replicon::server::visibility::filters_mask::FilterBit;
use bevy_replicon::server::visibility::registry::FilterRegistry;
use bevy_replicon::shared::replication::registry::ReplicationRegistry;
use lightyear_connection::client::Disconnected;
use serde::{Deserialize, Serialize};
use tracing::trace;
use crate::send::{ReplicationSender};

/// Marker component on the receiver side to indicate that the replicated entity
/// is under the control of the local peer that received the entity
#[derive(Component, Clone, PartialEq, Debug, Default, Reflect, Serialize, Deserialize)]
pub struct Controlled;


/// Component on the sender side that lists the entities controlled by the remote peer
#[derive(Component, Clone, PartialEq, Debug, Reflect)]
#[relationship_target(relationship = ControlledBy)]
#[reflect(Component)]
pub struct ControlledByRemote(Vec<Entity>);

/// Sender-side component that associates the entity with a [`ReplicationSender`] 'controlling' the entity
///
/// The receiver will add a [`Controlled`] marker component upon receiving the entity.
///
/// When the link is disconnected, the sender will optionally (based on the [`Lifetime`] value)
/// despawn the entity. If you want to persist an entity on the receiver side even after the link is disconnected,
/// see [`Persistent`](super::components::Persistent)
#[derive(Component, Clone, Copy, PartialEq, Debug, Reflect)]
// TODO: we add Controlled on the sender side to replicate it to the remote, but this could cause issues with client authority!
#[require(Controlled)]
#[reflect(Component)]
#[component(immutable)]
#[relationship(relationship_target = ControlledByRemote)]
pub struct ControlledBy {
    /// Which peer controls this entity? This should be an entity with a [`ReplicationSender`](crate::send::sender::ReplicationSender) component
    #[relationship]
    pub owner: Entity,
    /// What happens to the entity on the sender-side if the controlling client disconnects?
    pub lifetime: Lifetime,
}


impl ControlledBy {
    fn on_insert(trigger: On<Add, ControlledBy>, controlled_by: Query<&ControlledBy>, control_bit: Res<ControlBit>, mut sender: Query<&mut ClientVisibility, With<ReplicationSender>>) {
        let visibility_bit = control_bit.0;
        let sender_entity = controlled_by.get(trigger.entity).unwrap().owner;
        if let Ok(mut visibility) = sender.get_mut(sender_entity) {
            visibility.set(trigger.entity, visibility_bit, true);
        }
    }

    fn on_replace(trigger: On<Replace, ControlledBy>, controlled_by: Query<&ControlledBy>, control_bit: Res<ControlBit>, mut sender: Query<&mut ClientVisibility, With<ReplicationSender>>) {
        let visibility_bit = control_bit.0;
        let sender_entity = controlled_by.get(trigger.entity).unwrap().owner;
        if let Ok(mut visibility) = sender.get_mut(sender_entity) {
            visibility.set(trigger.entity, visibility_bit, false);
        }
    }

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


/// Component-level visibility for [`Controlled`]
#[derive(Resource, Deref)]
struct ControlBit(FilterBit);

impl FromWorld for ControlBit {
    fn from_world(world: &mut World) -> Self {
        let bit = world.resource_scope(|world, mut filter_registry: Mut<FilterRegistry>| {
            world.resource_scope(|world, mut registry: Mut<ReplicationRegistry>| {
                filter_registry.register_scope::<ComponentScope<Controlled>>(world, &mut registry)
            })
        });
        Self(bit)
    }
}

pub struct ControlPlugin;

impl Plugin for ControlPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ControlBit>();
        app.replicate::<Controlled>();
        app.add_observer(ControlledBy::on_insert);
        app.add_observer(ControlledBy::on_replace);
    }
}