use crate::send::ReplicationSender;
use alloc::vec::Vec;
use bevy_app::{App, Plugin};
use bevy_derive::Deref;
use bevy_ecs::prelude::*;
use bevy_reflect::Reflect;
use bevy_replicon::prelude::SingleComponent;
use bevy_replicon::server::visibility::client_visibility::ClientVisibility;
use bevy_replicon::server::visibility::filters_mask::FilterBit;
use bevy_replicon::server::visibility::registry::FilterRegistry;
use bevy_replicon::shared::replication::registry::ReplicationRegistry;
use lightyear_connection::client::Disconnected;
use lightyear_connection::host::HostClient;
use serde::{Deserialize, Serialize};
use tracing::trace;

// TODO: currently we add Controlled on the sender for replication but this could cause issues with authority changes.
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
    fn on_insert(
        trigger: On<Add, ControlledBy>,
        controlled_by: Query<&ControlledBy>,
        control_bit: Res<ControlBit>,
        mut sender: Query<(Entity, &mut ClientVisibility), With<ReplicationSender>>,
    ) {
        let visibility_bit = control_bit.0;
        let owner_entity = controlled_by.get(trigger.entity).unwrap().owner;
        // Two-pass: first hide Controlled for all clients, then show for owner only
        for (sender_entity, mut visibility) in sender.iter_mut() {
            if sender_entity == owner_entity {
                visibility.set(trigger.entity, visibility_bit, true);
            } else {
                visibility.set(trigger.entity, visibility_bit, false);
            }
        }
    }

    fn on_replace(
        trigger: On<Replace, ControlledBy>,
        controlled_by: Query<&ControlledBy>,
        control_bit: Res<ControlBit>,
        mut sender: Query<&mut ClientVisibility, With<ReplicationSender>>,
    ) {
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

/// Host-server local emulation for control when a client becomes a host client after entities
/// already exist.
fn emulate_controlled_on_host_client_added(
    trigger: On<Add, HostClient>,
    mut commands: Commands,
    controlled_by: Query<(Entity, &ControlledBy, Option<&Controlled>)>,
) {
    for (entity, controlled_by, controlled) in controlled_by.iter() {
        if controlled.is_none() && controlled_by.owner == trigger.entity {
            commands.entity(entity).insert(Controlled);
        }
    }
}

/// Host-server local emulation for control when a host-owned controlled entity is created.
fn emulate_controlled_on_add(
    trigger: On<Add, ControlledBy>,
    mut commands: Commands,
    controlled_by: Query<(&ControlledBy, Option<&Controlled>)>,
    host_clients: Query<(), With<HostClient>>,
) {
    let Ok((controlled_by, controlled)) = controlled_by.get(trigger.entity) else {
        return;
    };
    if controlled.is_none() && host_clients.get(controlled_by.owner).is_ok() {
        commands.entity(trigger.entity).insert(Controlled);
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
pub struct ControlBit(FilterBit);

impl FromWorld for ControlBit {
    fn from_world(world: &mut World) -> Self {
        let bit = world.resource_scope(|world, mut filter_registry: Mut<FilterRegistry>| {
            world.resource_scope(|world, mut registry: Mut<ReplicationRegistry>| {
                filter_registry.register_scope::<SingleComponent<Controlled>>(world, &mut registry)
            })
        });
        Self(bit)
    }
}

pub struct ControlPlugin;

impl Plugin for ControlPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ControlBit>();
        // Note: app.replicate::<Controlled>() is called in SharedComponentRegistrationPlugin
        // to ensure matching component IDs on both client and server.
        app.add_observer(ControlledBy::on_insert);
        app.add_observer(ControlledBy::on_replace);
        app.add_observer(ControlledBy::handle_disconnection);
        app.add_observer(emulate_controlled_on_host_client_added);
        app.add_observer(emulate_controlled_on_add);
    }
}
