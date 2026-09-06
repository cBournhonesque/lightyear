use alloc::vec::Vec;
use bevy_app::{App, Plugin};
use bevy_ecs::prelude::*;
use bevy_reflect::Reflect;
use bevy_replicon::bytes::Bytes;
use bevy_replicon::prelude::{
    AppVisibilityExt, RuleFns, ScopeLifetime, SingleComponent, VisibilityFilter,
};
use bevy_replicon::shared::replication::deferred_entity::DeferredEntity;
use bevy_replicon::shared::replication::registry::ctx::{RemoveCtx, WriteCtx};
use lightyear_connection::client::Disconnected;
use lightyear_connection::host::HostClient;
use lightyear_core::id::RemoteId;
use serde::{Deserialize, Serialize};
use tracing::trace;

/// Marker component indicating that the local peer controls this entity.
///
/// On the **client**, this is added to replicated entities that this client
/// "owns" (e.g. the player's own character). It is automatically inserted
/// by the replication system when the server sends a [`ControlledBy`]
/// component pointing to this client's link entity.
///
/// You can use `With<Controlled>` in queries to distinguish locally
/// controlled entities from remote ones.
#[derive(Component, Clone, PartialEq, Debug, Default, Reflect, Serialize, Deserialize)]
pub struct Controlled;

/// Sender-side marker that replicates as [`Controlled`] on the owning receiver.
///
/// This keeps [`Controlled`] receiver-local. In host-server mode, server-owned
/// entities for remote clients carry [`ControlledSend`] but not [`Controlled`].
#[derive(Component, Clone, PartialEq, Debug, Default, Reflect, Serialize, Deserialize)]
pub struct ControlledSend;

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
/// despawn the entity. To keep a received entity alive when its connection ends, use
/// [`Persistent`](crate::receive::Persistent) on the receiver side instead.
#[derive(Component, Clone, Copy, PartialEq, Debug, Reflect)]
#[require(ControlledSend)]
#[reflect(Component)]
#[component(immutable)]
#[relationship(relationship_target = ControlledByRemote)]
pub struct ControlledBy {
    /// Which peer controls this entity? This should be an entity with a [`ReplicationSender`] component.
    #[relationship]
    pub owner: Entity,
    /// What happens to the entity on the sender-side if the controlling client disconnects?
    pub lifetime: Lifetime,
}

/// `ControlledSend` is receiver-local: only the owning client sees it.
///
/// `RemoteId` is only the required client-component slot; the predicate uses
/// the link entity itself, so links without an id receive nothing.
impl VisibilityFilter for ControlledBy {
    type ClientComponent = RemoteId;
    type Scope = SingleComponent<ControlledSend>;
    const LIFETIME: ScopeLifetime = ScopeLifetime::WhileVisible;

    fn is_visible(&self, client: Entity, remote: Option<&RemoteId>) -> bool {
        remote.is_some_and(|_| client == self.owner)
    }
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

pub(crate) fn write_controlled(
    ctx: &mut WriteCtx,
    rule_fns: &RuleFns<ControlledSend>,
    entity: &mut DeferredEntity,
    message: &mut Bytes,
) -> bevy_ecs::error::Result<()> {
    let _ = rule_fns.deserialize(ctx, message)?;
    entity.insert(Controlled);
    Ok(())
}

pub(crate) fn remove_controlled(_ctx: &mut RemoveCtx, entity: &mut DeferredEntity) {
    entity.remove::<Controlled>();
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
    /// When the client that controls the entity disconnects, the entity is despawned on the sender.
    SessionBased,
    /// The entity is not despawned on the sender when the controlling client disconnects.
    Persistent,
}

pub struct ControlPlugin;

impl Plugin for ControlPlugin {
    fn build(&self, app: &mut App) {
        // `ControlledBy` is a `VisibilityFilter`: Replicon evaluates it on
        // insertion and on link changes, replacing the previous manual writes.
        // ControlledSend is registered in SharedComponentRegistrationPlugin
        // so its wire ID and custom Controlled receive behavior match.
        app.add_visibility_filter::<ControlledBy>();
        app.add_observer(ControlledBy::handle_disconnection);
        app.add_observer(emulate_controlled_on_host_client_added);
        app.add_observer(emulate_controlled_on_add);
    }
}
