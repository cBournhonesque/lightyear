//! Backend ownership and World-wide remote identity admission.
//!
//! Backends claim ownership when their marker is inserted and call [`admit`] only after resolving
//! both identities. A connection entity cannot change backend, even after disconnecting.
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
    let mut remote = world.get::<RemoteId>(entity).copied().unwrap_or(remote);
    if world.get::<P2P>(entity).is_none() {
        if endpoint.is_some_and(|endpoint| world.get::<Server>(endpoint).is_some()) {
            world.entity_mut(entity).insert(ClientOf);
            if world.get::<LocalId>(entity).is_none() {
                local = LocalId(PeerId::Server);
            }
        } else if world.get::<Client>(entity).is_some() && world.get::<RemoteId>(entity).is_none() {
            remote = RemoteId(PeerId::Server);
        }
    }
    admit(world, entity, local, remote, ())
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
    use lightyear_link::prelude::{LinkOf, Server};

    #[test]
    fn default_client_preserves_explicit_debug_identities() {
        let mut world = World::new();
        let local = LocalId(PeerId::Entity(1));
        let remote = RemoteId(PeerId::Entity(2));
        let entity = world.spawn((Client, local, remote)).id();
        assert!(apply_default_ids(
            &mut world,
            entity,
            LocalId(PeerId::Entity(3)),
            RemoteId(PeerId::Server),
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
            .spawn((
                P2P::default(),
                LocalId(PeerId::Local(10)),
                RemoteId(PeerId::Local(20)),
            ))
            .id();
        let fallback_local = LocalId(PeerId::Local(1));
        let fallback_remote = RemoteId(PeerId::Local(2));
        assert!(apply_default_ids(
            &mut world,
            entity,
            fallback_local,
            fallback_remote
        ));
        assert_eq!(
            world.get::<LocalId>(entity),
            Some(&LocalId(PeerId::Local(10)))
        );
        assert_eq!(
            world.get::<RemoteId>(entity),
            Some(&RemoteId(PeerId::Local(20)))
        );
        let duplicate = world
            .spawn((P2P::default(), RemoteId(PeerId::Local(20))))
            .id();
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
                .get(&PeerId::Local(20)),
            Some(&entity)
        );
    }

    #[test]
    fn endpoint_opt_out_blocks_default_admission() {
        let mut world = World::new();
        let endpoint = world.spawn(Server::default()).id();
        let entity = world.spawn(LinkOf { endpoint }).id();
        assert!(is_default_connection(&world, entity));
        world.entity_mut(endpoint).insert(NoDefaultConnection);
        assert!(!apply_default_ids(
            &mut world,
            entity,
            LocalId(PeerId::Local(1)),
            RemoteId(PeerId::Local(2)),
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
        assert_eq!(local.0, PeerId::Local(10));
        assert_eq!(remote.0, PeerId::Local(20));
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
            LocalId(PeerId::Local(10)),
            RemoteId(PeerId::Local(20)),
            ()
        ));
        assert!(!admit(
            app.world_mut(),
            right,
            LocalId(PeerId::Local(30)),
            RemoteId(PeerId::Local(20)),
            ()
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
                .get(&PeerId::Local(20)),
            Some(&left)
        );
    }

    #[test]
    fn removal_and_despawn_release_only_the_current_owner() {
        let mut world = World::new();
        let old = world.spawn_empty().id();
        let remote = PeerId::Local(20);
        assert!(admit(
            &mut world,
            old,
            LocalId(PeerId::Local(10)),
            RemoteId(remote),
            ()
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
            LocalId(PeerId::Local(10)),
            RemoteId(remote),
            ()
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
        let remote_server = world.spawn_empty().id();
        assert!(!admit(
            &mut world,
            remote_server,
            LocalId(PeerId::Local(10)),
            RemoteId(PeerId::Server),
            ()
        ));
        assert_eq!(
            world
                .resource::<NetworkingMetadata>()
                .peer_map
                .get(&PeerId::Server),
            Some(&server)
        );
        world.despawn(server);
        assert!(admit(
            &mut world,
            remote_server,
            LocalId(PeerId::Local(10)),
            RemoteId(PeerId::Server),
            ()
        ));
        let second_server = world.spawn(Server::default()).id();
        assert!(!crate::server::start_server(&mut world, second_server));
        world.flush();
        assert!(world.get::<crate::server::Stopped>(second_server).is_some());
        assert!(world.get::<Disconnected>(second_server).is_some());
        assert_eq!(
            world
                .resource::<NetworkingMetadata>()
                .peer_map
                .get(&PeerId::Server),
            Some(&remote_server)
        );
    }
}
