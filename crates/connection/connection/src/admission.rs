//! Backend ownership and World-wide remote identity admission.
//!
//! Backends claim ownership when their marker is inserted and call [`admit`] only after resolving
//! both identities. A connection entity cannot change backend, even after disconnecting.
use alloc::{format, string::String};
use bevy_ecs::{lifecycle::HookContext, prelude::*, world::DeferredWorld};
use lightyear_core::id::{LocalId, RemoteId};
use lightyear_link::prelude::{LinkOf, Server, Unlink, UnlinkReason};

use crate::{
    client::{Client, Connected, Connecting, Disconnected, DisconnectedReason, Disconnecting},
    network_topology::NetworkingMetadata,
    p2p::P2P,
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
