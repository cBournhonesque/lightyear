//! Fan-out relationships: an endpoint owns many link entities.
//!
//! An [`Endpoint`] is the entity that other peers connect *to*. It does not carry a link itself —
//! [`Link`] and its buffers live on the children — and it is deliberately not tied to any role:
//! a peer in a P2P session, a game server, and anything in between are all endpoints. Roles are
//! separate markers layered on top, such as [`Server`](crate::server::Server).
//!
//! The relationship is modelled with Bevy's API: [`Endpoint`] is the relationship target, and
//! [`LinkOf`] is inserted on each child link entity to point back to the endpoint. Transports use
//! this to keep the endpoint independent from the concrete links used for each peer.

use crate::{
    Link, LinkPlugin, Linked, Linking, RecvLinkConditioner, Unlink, UnlinkReason, Unlinked,
};
use alloc::{format, vec::Vec};
use bevy_app::{App, Plugin};
use bevy_ecs::lifecycle::HookContext;
use bevy_ecs::prelude::*;
use bevy_ecs::{
    relationship::{
        Relationship, RelationshipHookMode, RelationshipSourceCollection, RelationshipTarget,
    },
    world::DeferredWorld,
};
use bevy_reflect::Reflect;
use bevy_utils::prelude::DebugName;
use lightyear_core::time::Instant;
#[allow(unused_imports)]
use tracing::{trace, warn};

/// Relationship target for an endpoint that owns multiple link entities.
///
/// Insert this on the entity that represents a listening or hosting endpoint. Entities with
/// [`LinkOf`] are collected under this component, allowing systems to find and tear down all child
/// links when the endpoint goes away.
/// The target collection uses `linked_spawn`, so spawning a link with [`LinkOf`] can establish
/// the relationship at spawn time.
#[derive(Component, Default, Debug, Reflect)]
#[component(on_add = Endpoint::on_add)]
#[relationship_target(relationship = LinkOf, linked_spawn)]
pub struct Endpoint {
    #[relationship]
    links: Vec<Entity>,
    /// Receive conditioner cloned into each new [`LinkOf`] child.
    ///
    /// The endpoint does not receive packets itself. This conditioner is a template; each child link
    /// receives an independent clone whose runtime state lives in [`Link::recv`].
    #[reflect(ignore)]
    pub conditioner: Option<RecvLinkConditioner>,
}

impl Endpoint {
    /// Creates an endpoint with an optional receive conditioner for its child links.
    pub fn new(conditioner: Option<RecvLinkConditioner>) -> Self {
        Self {
            links: Vec::new(),
            conditioner,
        }
    }

    fn on_add(mut world: DeferredWorld, context: HookContext) {
        let entity_ref = world.entity(context.entity);
        if !entity_ref.contains::<Unlinked>()
            && !entity_ref.contains::<Linked>()
            && !entity_ref.contains::<Linking>()
        {
            trace!("Inserting Unlinked because Endpoint was added");
            world.commands().entity(context.entity).insert(Unlinked {
                reason: UnlinkReason::Initial,
            });
        };
    }

    fn unlinked(
        trigger: On<Add<Unlinked>>,
        mut query: Query<(&Endpoint, &Unlinked)>,
        mut commands: Commands,
    ) {
        if let Ok((endpoint, unlinked)) = query.get_mut(trigger.entity) {
            for link_of in endpoint.collection() {
                commands.trigger(Unlink {
                    entity: *link_of,
                    reason: unlinked.reason.clone(),
                });
                if let Ok(mut c) = commands.get_entity(*link_of) {
                    // cannot simply insert Unlinked because then we wouldn't close aeronet sessions...
                    trace!("Despawning link entity because its endpoint became unlinked");
                    c.try_despawn();
                }
            }
        }
    }
}

/// Relationship source component for a link that belongs to an [`Endpoint`].
///
/// Insert this on a per-peer link entity and set [`endpoint`](Self::endpoint) to the endpoint
/// entity.
/// The custom relationship hooks keep the [`Endpoint`] collection up to date without despawning the
/// endpoint entity when the last link is removed.
#[derive(Component, Clone, Copy, PartialEq, Eq, Debug, Reflect)]
#[component(on_insert = LinkOf::on_insert_hook)]
#[component(on_discard = LinkOf::on_discard)]
pub struct LinkOf {
    /// Endpoint that owns this link entity.
    pub endpoint: Entity,
}

impl Relationship for LinkOf {
    type RelationshipTarget = Endpoint;
    #[inline(always)]
    fn get(&self) -> Entity {
        self.endpoint
    }
    #[inline]
    fn from(entity: Entity) -> Self {
        Self { endpoint: entity }
    }

    fn set_risky(&mut self, entity: Entity) {
        self.endpoint = entity;
    }
}

impl LinkOf {
    fn on_insert_hook(
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
            RelationshipHookMode::RunIfNotLinked => return,
        }
        let target_entity = world.entity(entity).get::<Self>().unwrap().get();
        if target_entity == entity {
            warn!(
                "{}The {}({target_entity:?}) relationship on entity {entity:?} points to itself. The invalid {} relationship has been removed.",
                caller
                    .map(|location| format!("{location}: "))
                    .unwrap_or_default(),
                DebugName::type_name::<Self>(),
                DebugName::type_name::<Self>()
            );
            world.commands().entity(entity).remove::<Self>();
            return;
        }
        if let Ok(mut target_entity_mut) = world.get_entity_mut(target_entity) {
            if let Some(mut relationship_target) = target_entity_mut.get_mut::<Endpoint>() {
                relationship_target.collection_mut_risky().add(entity);
            } else {
                let mut target = <Endpoint as RelationshipTarget>::with_capacity(1);
                target.collection_mut_risky().add(entity);
                world.commands().entity(target_entity).insert(target);
            }
        } else {
            warn!(
                "{}The {}({target_entity:?}) relationship on entity {entity:?} relates to an entity that does not exist. The invalid {} relationship has been removed.",
                caller
                    .map(|location| format!("{location}: "))
                    .unwrap_or_default(),
                DebugName::type_name::<Self>(),
                DebugName::type_name::<Self>()
            );
            world.commands().entity(entity).remove::<Self>();
        }
    }

    fn on_discard(
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
                if <Endpoint as RelationshipTarget>::LINKED_SPAWN {
                    return;
                }
            }
        }
        let target_entity = world.entity(entity).get::<Self>().unwrap().get();
        if let Ok(mut target_entity_mut) = world.get_entity_mut(target_entity)
            && let Some(mut relationship_target) = target_entity_mut.get_mut::<Endpoint>()
        {
            RelationshipSourceCollection::remove(
                relationship_target.collection_mut_risky(),
                entity,
            );
        }
    }
}

/// Copies an endpoint's receive conditioner into each newly-created link.
///
/// An endpoint is only the discovery/acceptance point; packets are received by its [`LinkOf`] child
/// entities. Keeping the conditioner in [`Link::recv`] lets all IO backends use their existing
/// receive path unchanged.
fn add_endpoint_link_conditioner(
    trigger: On<Add<LinkOf>>,
    mut links: Query<(&LinkOf, &mut Link)>,
    endpoints: Query<&Endpoint>,
) {
    let Ok((link_of, mut link)) = links.get_mut(trigger.entity) else {
        return;
    };
    let Ok(endpoint) = endpoints.get(link_of.endpoint) else {
        return;
    };
    let Some(conditioner) = &endpoint.conditioner else {
        return;
    };
    if link.recv.conditioner.is_some() {
        return;
    }

    // The link can receive packets before deferred observers run. Reinsert any such packets so
    // that they are conditioned too.
    let queued_packets: Vec<_> = link.recv.drain().collect();
    link.recv.conditioner = Some(conditioner.clone());
    for packet in queued_packets {
        link.recv.push(packet, Instant::now());
    }
}

/// Plugin that installs endpoint/link relationship support.
///
/// The plugin ensures [`LinkPlugin`] is present and adds the observers that react to [`Unlinked`] on
/// endpoint entities by unlinking/despawning their child links, and that copy the endpoint's
/// conditioner into each new child. Transports that expose a multi-link endpoint add this plugin
/// before scheduling their IO systems.
pub struct EndpointLinkPlugin;

impl Plugin for EndpointLinkPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<LinkPlugin>() {
            app.add_plugins(LinkPlugin);
        }
        app.add_observer(Endpoint::unlinked);
        app.add_observer(add_endpoint_link_conditioner);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conditioner::LinkConditionerConfig;
    use crate::{Link, Linked};
    use core::time::Duration;

    #[derive(Resource, Default)]
    struct UnlinkedChildren(Vec<Entity>);

    fn record_unlink(trigger: On<Unlink>, mut unlinked: ResMut<UnlinkedChildren>) {
        unlinked.0.push(trigger.entity);
    }

    #[test]
    fn endpoint_unlinked_triggers_unlink_for_child_links() {
        let mut app = App::new();
        app.add_plugins(EndpointLinkPlugin);
        app.init_resource::<UnlinkedChildren>();
        app.add_observer(record_unlink);

        let endpoint = app.world_mut().spawn((Endpoint::default(), Linked)).id();
        let child = app
            .world_mut()
            .spawn((LinkOf { endpoint }, Link::default(), Linked))
            .id();

        app.world_mut().entity_mut(endpoint).insert(Unlinked {
            reason: UnlinkReason::ServerStopped,
        });
        app.update();

        let unlinked = &app.world().resource::<UnlinkedChildren>().0;
        assert_eq!(unlinked, &[child]);
    }

    #[test]
    fn link_of_inherits_endpoint_conditioner() {
        let mut app = App::new();
        app.add_plugins(EndpointLinkPlugin);
        let endpoint = app
            .world_mut()
            .spawn(Endpoint::new(Some(RecvLinkConditioner::new(
                LinkConditionerConfig {
                    incoming_latency: Duration::from_millis(100),
                    ..Default::default()
                },
            ))))
            .id();

        let link = app
            .world_mut()
            .spawn((LinkOf { endpoint }, Link::default()))
            .id();

        assert!(
            app.world()
                .entity(link)
                .get::<Link>()
                .unwrap()
                .recv
                .conditioner
                .is_some()
        );
    }
}
