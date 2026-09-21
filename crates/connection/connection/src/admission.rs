//! World-wide remote identity admission and default connection support.
//!
//! Explicit backends opt out of default admission with [`NoDefaultConnection`] and call
//! [`admit`] after resolving both identities and completing authentication.
use alloc::{format, string::String};
use bevy_app::{App, Plugin};
use bevy_ecs::{lifecycle::HookContext, prelude::*, world::DeferredWorld};
use lightyear_core::id::{LocalId, PeerId, RemoteId};
use lightyear_link::{
    Linked,
    prelude::{LinkOf, Server, Unlink, UnlinkReason},
};

use crate::{
    client::{
        Client, Connected, Connecting, Disconnect, Disconnected, DisconnectedReason, Disconnecting,
    },
    client_of::ClientOf,
    network_topology::NetworkingMetadata,
    p2p::P2P,
    server::{Stop, start_server},
};

/// Prevent implicit selection of the default backend on this entity or its endpoint's children.
#[derive(Component, Default, Debug)]
pub struct NoDefaultConnection;

pub fn is_default_connection(world: &World, entity: Entity) -> bool {
    let Ok(link) = world.get_entity(entity) else {
        return false;
    };
    let endpoint = link.get::<LinkOf>().map(|link| link.endpoint);
    !link.contains::<NoDefaultConnection>()
        && !endpoint.is_some_and(|endpoint| world.get::<NoDefaultConnection>(endpoint).is_some())
        && !(link.contains::<Client>()
            && !link.contains::<P2P>()
            && endpoint.is_some_and(|endpoint| world.get::<Server>(endpoint).is_some()))
}

/// Admit an unowned debug link using native identities as fallbacks.
///
/// Requested IDs stay local to this World; this does not negotiate identities with the peer.
/// Client/server roles default to the server sentinel; explicitly configured debug IDs are preserved.
/// An occupied, unrequested non-P2P debug alias may fall back locally; shared or requested IDs never do.
pub fn apply_default_ids(
    world: &mut World,
    entity: Entity,
    local: LocalId,
    remote: RemoteId,
) -> bool {
    if !is_default_connection(world, entity) {
        return false;
    }
    let endpoint = world.get::<LinkOf>(entity).map(|link| link.endpoint);
    let mut local = world.get::<LocalId>(entity).copied().unwrap_or(local);
    let requested_remote = world.get::<RemoteId>(entity).copied();
    let mut remote = requested_remote.unwrap_or(remote);
    let p2p = world.get::<P2P>(entity).is_some();
    if !p2p {
        if endpoint.is_some_and(|endpoint| world.get::<Server>(endpoint).is_some()) {
            world.entity_mut(entity).insert(ClientOf);
            if world.get::<LocalId>(entity).is_none() {
                local = LocalId(PeerId::SERVER);
            }
        } else if world.get::<Client>(entity).is_some() && requested_remote.is_none() {
            remote = RemoteId(PeerId::SERVER);
        }
    }
    if !p2p && requested_remote.is_none() && remote.0 != PeerId::SERVER {
        remote.0 = free_local_alias(world, entity, remote.0);
    }
    admit(world, entity, local, remote, ())
}

/// Find a free local-only label, preserving the candidate unless it collides.
pub(crate) fn free_local_alias(world: &World, entity: Entity, candidate: PeerId) -> PeerId {
    debug_assert_ne!(candidate, PeerId::SERVER);
    let Some(metadata) = world.get_resource::<NetworkingMetadata>() else {
        return candidate;
    };
    let mut alias = candidate;
    while metadata
        .peer_map
        .get(&alias)
        .is_some_and(|owner| *owner != entity)
    {
        alias.0 = alias.0.wrapping_add(1).max(1);
    }
    alias
}

/// Connection lifecycle support installed by transports that support default admission.
pub struct DefaultConnectionPlugin;

impl DefaultConnectionPlugin {
    fn disconnect(trigger: On<Disconnect>, mut commands: Commands) {
        let entity = trigger.entity;
        commands.queue(move |world: &mut World| {
            if is_default_connection(world, entity) {
                world.trigger(Unlink {
                    entity,
                    reason: UnlinkReason::UserRequested(None),
                });
            }
        });
    }

    fn start(trigger: On<Add, Linked>, mut commands: Commands) {
        let entity = trigger.entity;
        commands.queue(move |world: &mut World| {
            if world.get::<Server>(entity).is_some() && is_default_connection(world, entity) {
                start_server(world, entity);
            }
        });
    }

    fn stop(trigger: On<Stop>, mut commands: Commands) {
        let entity = trigger.entity;
        commands.queue(move |world: &mut World| {
            if is_default_connection(world, entity) {
                world.trigger(Unlink {
                    entity,
                    reason: UnlinkReason::ServerStopped,
                });
            }
        });
    }
}

impl Plugin for DefaultConnectionPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<crate::client::ConnectionPlugin>() {
            app.add_plugins(crate::client::ConnectionPlugin);
        }
        if !app.is_plugin_added::<crate::server::ConnectionPlugin>() {
            app.add_plugins(crate::server::ConnectionPlugin);
        }
        app.add_observer(Self::disconnect);
        app.add_observer(Self::start);
        app.add_observer(Self::stop);
    }
}

/// Install resolved identities and register their routing binding before exposing [`Connected`].
/// Duplicate remote identities are rejected across all endpoints and backends in this World.
pub fn admit<B: Bundle>(
    world: &mut World,
    entity: Entity,
    local: LocalId,
    remote: RemoteId,
    components: B,
) -> bool {
    let p2p = world.get::<P2P>(entity).is_some();
    let server_child = world.get::<ClientOf>(entity).is_some()
        || world
            .get::<LinkOf>(entity)
            .is_some_and(|link| world.get::<Server>(link.endpoint).is_some());
    let host = server_child && world.get::<Client>(entity).is_some();
    let local_server = !p2p && !host && server_child;
    let remote_server = !p2p && !host && !server_child && world.get::<Client>(entity).is_some();
    if (local.0 == PeerId::SERVER && !local_server)
        || (remote.0 == PeerId::SERVER && !remote_server)
    {
        reject(
            world,
            entity,
            String::from("Peer ID zero is reserved for the client/server server role"),
        );
        return false;
    }
    world.init_resource::<NetworkingMetadata>();
    if let Some(owner) = world
        .resource::<NetworkingMetadata>()
        .peer_map
        .get(&remote.0)
        .copied()
        && owner != entity
    {
        reject(
            world,
            entity,
            format!(
                "Remote identity {:?} is already registered to {:?}",
                remote.0, owner
            ),
        );
        return false;
    }
    if world.get::<Connected>(entity).is_some() {
        if world
            .get::<LocalId>(entity)
            .is_some_and(|id| id.0 == local.0)
            && world
                .get::<RemoteId>(entity)
                .is_some_and(|id| id.0 == remote.0)
        {
            return true;
        }
        reject(
            world,
            entity,
            String::from("Cannot change the identity of a connected entity"),
        );
        return false;
    }
    world
        .entity_mut(entity)
        .remove::<(Connecting, Disconnecting, Disconnected)>();
    world.entity_mut(entity).insert((local, remote));
    world
        .resource_mut::<NetworkingMetadata>()
        .peer_map
        .insert(remote.0, entity);
    world.entity_mut(entity).insert((Connected, components));
    true
}

/// Fail admission with an observable reason and close any underlying transport.
pub fn reject(world: &mut World, entity: Entity, reason: String) {
    if world.get_entity(entity).is_err() {
        return;
    }
    world
        .entity_mut(entity)
        .remove::<(Connected, Connecting, Disconnecting)>();
    world.entity_mut(entity).insert(Disconnected {
        reason: DisconnectedReason::TransportError(reason.clone()),
    });
    world.trigger(Unlink {
        entity,
        reason: UnlinkReason::TransportError(reason),
    });
}

/// Release a binding only while this entity still owns it.
pub(crate) fn release(mut world: DeferredWorld, context: HookContext) {
    if let Some(remote) = world.get::<RemoteId>(context.entity).map(|id| id.0)
        && let Some(mut metadata) = world.get_resource_mut::<NetworkingMetadata>()
        && metadata.peer_map.get(&remote) == Some(&context.entity)
    {
        metadata.peer_map.remove(&remote);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy_app::App;
    use lightyear_core::id::PeerId;

    #[test]
    fn default_client_preserves_explicit_debug_identities() {
        let mut world = World::new();
        let local = LocalId(PeerId(1));
        let remote = RemoteId(PeerId(2));
        let entity = world.spawn((Client, local, remote)).id();
        assert!(apply_default_ids(
            &mut world,
            entity,
            LocalId(PeerId(3)),
            RemoteId(PeerId::SERVER),
        ));
        assert_eq!(world.get::<LocalId>(entity), Some(&local));
        assert_eq!(world.get::<RemoteId>(entity), Some(&remote));
        assert_eq!(
            world
                .resource::<NetworkingMetadata>()
                .peer_map
                .get(&remote.0),
            Some(&entity)
        );
    }

    #[test]
    fn p2p_requested_ids_are_preserved_and_conflicts_rejected() {
        let mut world = World::new();
        let entity = world
            .spawn((P2P::default(), LocalId(PeerId(10)), RemoteId(PeerId(20))))
            .id();
        let fallback_local = LocalId(PeerId(1));
        let fallback_remote = RemoteId(PeerId(2));
        assert!(apply_default_ids(
            &mut world,
            entity,
            fallback_local,
            fallback_remote
        ));
        assert_eq!(world.get::<LocalId>(entity), Some(&LocalId(PeerId(10))));
        assert_eq!(world.get::<RemoteId>(entity), Some(&RemoteId(PeerId(20))));
        let duplicate = world.spawn((P2P::default(), RemoteId(PeerId(20)))).id();
        assert!(!apply_default_ids(
            &mut world,
            duplicate,
            fallback_local,
            fallback_remote
        ));
        assert!(world.get::<Connected>(duplicate).is_none());
        assert!(world.get::<Disconnected>(duplicate).is_some());
        assert_eq!(
            world
                .resource::<NetworkingMetadata>()
                .peer_map
                .get(&PeerId(20)),
            Some(&entity)
        );
    }

    #[test]
    fn endpoint_opt_out_blocks_default_admission() {
        let mut world = World::new();
        let endpoint = world.spawn(Server::default()).id();
        let entity = world.spawn(LinkOf { endpoint }).id();
        world.entity_mut(endpoint).insert(NoDefaultConnection);
        assert!(!apply_default_ids(
            &mut world,
            entity,
            LocalId(PeerId(1)),
            RemoteId(PeerId(2)),
        ));
        assert!(world.get::<Connected>(entity).is_none());
        assert!(world.get::<ClientOf>(entity).is_none());
        assert!(world.get::<LocalId>(entity).is_none());
        assert!(world.get::<RemoteId>(entity).is_none());
    }

    #[derive(Resource, Default)]
    struct Admissions(usize);

    fn observe_admission(
        trigger: On<Add, Connected>,
        query: Query<(&LocalId, &RemoteId, Has<Connecting>)>,
        metadata: Res<NetworkingMetadata>,
        mut admissions: ResMut<Admissions>,
    ) {
        let (local, remote, connecting) = query.get(trigger.entity).unwrap();
        assert_eq!(local.0, PeerId(10));
        assert_eq!(remote.0, PeerId(20));
        assert!(!connecting);
        assert_eq!(metadata.peer_map.get(&remote.0), Some(&trigger.entity));
        admissions.0 += 1;
    }

    #[test]
    fn duplicate_across_endpoints_never_notifies_connected_observers() {
        let mut app = App::new();
        app.init_resource::<Admissions>();
        app.add_observer(observe_admission);
        let left_endpoint = app.world_mut().spawn_empty().id();
        let right_endpoint = app.world_mut().spawn_empty().id();
        let left = app
            .world_mut()
            .spawn((
                LinkOf {
                    endpoint: left_endpoint,
                },
                Connecting,
            ))
            .id();
        let right = app
            .world_mut()
            .spawn(LinkOf {
                endpoint: right_endpoint,
            })
            .id();
        app.world_mut().flush();
        assert!(admit(
            app.world_mut(),
            left,
            LocalId(PeerId(10)),
            RemoteId(PeerId(20)),
            (),
        ));
        assert!(!admit(
            app.world_mut(),
            right,
            LocalId(PeerId(30)),
            RemoteId(PeerId(20)),
            (),
        ));
        app.world_mut().flush();
        assert_eq!(app.world().resource::<Admissions>().0, 1);
        assert!(app.world().get::<Connected>(right).is_none());
        assert!(matches!(
            app.world().get::<Disconnected>(right).unwrap().reason,
            DisconnectedReason::TransportError(_)
        ));
        assert_eq!(
            app.world()
                .resource::<NetworkingMetadata>()
                .peer_map
                .get(&PeerId(20)),
            Some(&left)
        );
    }

    #[test]
    fn removal_and_despawn_release_only_the_current_owner() {
        let mut world = World::new();
        let old = world.spawn_empty().id();
        let remote = PeerId(20);
        assert!(admit(
            &mut world,
            old,
            LocalId(PeerId(10)),
            RemoteId(remote),
            (),
        ));
        world.entity_mut(old).remove::<Connected>();
        assert!(
            !world
                .resource::<NetworkingMetadata>()
                .peer_map
                .contains_key(&remote)
        );
        let replacement = world.spawn_empty().id();
        assert!(admit(
            &mut world,
            replacement,
            LocalId(PeerId(10)),
            RemoteId(remote),
            (),
        ));
        world.despawn(old);
        assert_eq!(
            world.resource::<NetworkingMetadata>().peer_map.get(&remote),
            Some(&replacement)
        );
        world.despawn(replacement);
        assert!(
            !world
                .resource::<NetworkingMetadata>()
                .peer_map
                .contains_key(&remote)
        );
    }

    #[test]
    fn local_server_and_remote_server_cannot_share_the_server_route() {
        let mut world = World::new();
        world.init_resource::<NetworkingMetadata>();
        let server = world
            .spawn((Server::default(), crate::server::Started))
            .id();
        let remote_server = world.spawn(Client).id();
        assert!(!admit(
            &mut world,
            remote_server,
            LocalId(PeerId(10)),
            RemoteId(PeerId::SERVER),
            (),
        ));
        assert_eq!(
            world
                .resource::<NetworkingMetadata>()
                .peer_map
                .get(&PeerId::SERVER),
            Some(&server)
        );
        world.despawn(server);
        assert!(admit(
            &mut world,
            remote_server,
            LocalId(PeerId(10)),
            RemoteId(PeerId::SERVER),
            (),
        ));
        let second_server = world.spawn(Server::default()).id();
        assert!(!start_server(&mut world, second_server));
        world.flush();
        assert!(world.get::<crate::server::Stopped>(second_server).is_some());
        assert!(world.get::<Disconnected>(second_server).is_some());
        assert_eq!(
            world
                .resource::<NetworkingMetadata>()
                .peer_map
                .get(&PeerId::SERVER),
            Some(&remote_server)
        );
    }

    #[test]
    fn local_fallback_wraps_without_reassigning_shared_or_requested_aliases() {
        let mut world = World::new();
        let owner = world.spawn_empty().id();
        assert!(admit(
            &mut world,
            owner,
            LocalId(PeerId(5)),
            RemoteId(PeerId(u64::MAX)),
            (),
        ));
        let debug = world.spawn_empty().id();
        assert!(apply_default_ids(
            &mut world,
            debug,
            LocalId(PeerId(6)),
            RemoteId(PeerId(u64::MAX))
        ));
        assert_eq!(world.get::<RemoteId>(debug), Some(&RemoteId(PeerId(1))));
        assert_eq!(
            world
                .resource::<NetworkingMetadata>()
                .peer_map
                .get(&PeerId(u64::MAX)),
            Some(&owner)
        );
        let requested = world.spawn(RemoteId(PeerId(u64::MAX))).id();
        assert!(!apply_default_ids(
            &mut world,
            requested,
            LocalId(PeerId(7)),
            RemoteId(PeerId(8))
        ));
        let shared = world.spawn(P2P::default()).id();
        assert!(!apply_default_ids(
            &mut world,
            shared,
            LocalId(PeerId(7)),
            RemoteId(PeerId(u64::MAX))
        ));
        assert!(world.get::<Connected>(requested).is_none());
        assert!(world.get::<Connected>(shared).is_none());
        world.despawn(debug);
        let replacement = world.spawn_empty().id();
        assert!(apply_default_ids(
            &mut world,
            replacement,
            LocalId(PeerId(6)),
            RemoteId(PeerId(u64::MAX))
        ));
        assert_eq!(
            world.get::<RemoteId>(replacement),
            Some(&RemoteId(PeerId(1)))
        );
    }

    #[test]
    fn reserved_identity_requires_the_corresponding_non_p2p_server_role() {
        let mut world = World::new();
        let bare = world.spawn_empty().id();
        assert!(!admit(
            &mut world,
            bare,
            LocalId(PeerId(5)),
            RemoteId(PeerId::SERVER),
            (),
        ));
        let client = world.spawn(Client).id();
        assert!(!admit(
            &mut world,
            client,
            LocalId(PeerId::SERVER),
            RemoteId(PeerId(5)),
            (),
        ));
        assert!(admit(
            &mut world,
            client,
            LocalId(PeerId(5)),
            RemoteId(PeerId::SERVER),
            (),
        ));
        let endpoint = world.spawn(Server::default()).id();
        let child = world.spawn((ClientOf, LinkOf { endpoint })).id();
        assert!(admit(
            &mut world,
            child,
            LocalId(PeerId::SERVER),
            RemoteId(PeerId(6)),
            (),
        ));
        for (local, remote) in [(PeerId::SERVER, PeerId(7)), (PeerId(7), PeerId::SERVER)] {
            let p2p = world
                .spawn((P2P::default(), ClientOf, LinkOf { endpoint }))
                .id();
            assert!(!admit(
                &mut world,
                p2p,
                LocalId(local),
                RemoteId(remote),
                (),
            ));
            assert!(world.get::<Connected>(p2p).is_none());
        }
    }
}
