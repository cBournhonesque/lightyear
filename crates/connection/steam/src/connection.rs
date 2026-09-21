use bevy_app::{App, Plugin};
use bevy_ecs::prelude::*;
use lightyear_connection::admission::{NoDefaultConnection, admit, reject};
use lightyear_connection::client::{Connected, Disconnect};
#[cfg(all(feature = "server", not(target_family = "wasm")))]
use lightyear_connection::server::{Stop, start_server};
use lightyear_core::id::{LocalId, PeerId, RemoteId};
use lightyear_link::endpoint::LinkOf;
use lightyear_link::server::Server;
use lightyear_link::{Linked, Unlink, UnlinkReason};

use crate::SteamSessionFacts;

/// Admits established Steam sessions using Steam identity or an address client's server role.
///
/// Add this plugin only when Steam is the selected connection backend. Use
/// [`crate::SteamTransportPlugin`] alone when another backend (for example netcode) owns admission.
pub struct SteamConnectionPlugin;

impl Plugin for SteamConnectionPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<lightyear_connection::client::ConnectionPlugin>() {
            app.add_plugins(lightyear_connection::client::ConnectionPlugin);
        }
        #[cfg(feature = "server")]
        if !app.is_plugin_added::<lightyear_connection::server::ConnectionPlugin>() {
            app.add_plugins(lightyear_connection::server::ConnectionPlugin);
        }
        #[cfg(feature = "client")]
        {
            app.register_required_components::<crate::client::SteamClientIo, NoDefaultConnection>();
            app.register_required_components::<
                crate::client::SteamClientIo,
                lightyear_connection::client::Client,
            >();
        }
        #[cfg(all(any(feature = "p2p", feature = "server"), not(target_family = "wasm")))]
        app.register_required_components::<crate::endpoint::SteamEndpoint, NoDefaultConnection>();
        app.register_required_components::<SteamSessionFacts, NoDefaultConnection>();
        app.add_observer(Self::session_ready);
        app.add_observer(Self::disconnect);
        #[cfg(all(feature = "server", not(target_family = "wasm")))]
        {
            app.add_observer(Self::endpoint_linked);
            app.add_observer(Self::stop);
        }
    }
}

impl SteamConnectionPlugin {
    fn session_ready(
        trigger: On<Add, (SteamSessionFacts, Linked)>,
        query: Query<&SteamSessionFacts, (With<Linked>, Without<Connected>)>,
        mut commands: Commands,
    ) {
        if let Ok(&facts) = query.get(trigger.entity) {
            let entity = trigger.entity;
            commands.queue(move |world: &mut World| {
                let endpoint = world.get::<LinkOf>(entity).map(|link| link.endpoint);
                let p2p = world
                    .get::<lightyear_connection::p2p::P2P>(entity)
                    .is_some()
                    || endpoint.is_some_and(|endpoint| {
                        world
                            .get::<lightyear_connection::p2p::P2P>(endpoint)
                            .is_some()
                    });
                let server_child = !p2p
                    && endpoint.is_some_and(|endpoint| world.get::<Server>(endpoint).is_some());
                if facts.local.raw() == 0 || facts.remote.is_some_and(|id| id.raw() == 0) {
                    reject(
                        world,
                        entity,
                        "Steam identity zero is reserved for the server role".into(),
                    );
                    return;
                }
                if facts.remote.is_none() && (p2p || server_child) {
                    reject(
                        world,
                        entity,
                        "Steam address sessions cannot authenticate a remote peer identity".into(),
                    );
                    return;
                }
                let local = if server_child {
                    PeerId::SERVER
                } else {
                    PeerId(facts.local.raw())
                };
                let remote = facts.remote.map_or(PeerId::SERVER, |id| PeerId(id.raw()));
                if world.get::<LocalId>(entity).is_some_and(|id| id.0 != local)
                    || world
                        .get::<RemoteId>(entity)
                        .is_some_and(|id| id.0 != remote)
                {
                    reject(
                        world,
                        entity,
                        "Requested identity does not match authenticated Steam identity".into(),
                    );
                    return;
                }
                admit(world, entity, LocalId(local), RemoteId(remote), ());
            });
        }
    }

    fn disconnect(
        trigger: On<Disconnect>,
        query: Query<(), With<SteamSessionFacts>>,
        #[cfg(feature = "client")] clients: Query<(), With<crate::client::SteamClientIo>>,
        mut commands: Commands,
    ) {
        let steam = query.contains(trigger.entity);
        #[cfg(feature = "client")]
        let steam = steam || clients.contains(trigger.entity);
        if steam {
            commands.trigger(Unlink {
                entity: trigger.entity,
                reason: UnlinkReason::UserRequested(None),
            });
        }
    }

    #[cfg(all(feature = "server", not(target_family = "wasm")))]
    fn endpoint_linked(
        trigger: On<Add, Linked>,
        query: Query<(), (With<crate::endpoint::SteamEndpoint>, With<Server>)>,
        mut commands: Commands,
    ) {
        if query.contains(trigger.entity) {
            let entity = trigger.entity;
            commands.queue(move |world: &mut World| {
                start_server(world, entity);
            });
        }
    }

    #[cfg(all(feature = "server", not(target_family = "wasm")))]
    fn stop(
        trigger: On<Stop>,
        query: Query<(), With<crate::endpoint::SteamEndpoint>>,
        mut commands: Commands,
    ) {
        if query.contains(trigger.entity) {
            commands.trigger(Unlink {
                entity: trigger.entity,
                reason: UnlinkReason::UserRequested(None),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aeronet_steam::steamworks::SteamId;
    use lightyear_connection::client::Disconnected;
    use lightyear_connection::network_topology::NetworkingMetadata;
    use lightyear_link::{Link, Unlinked};

    fn facts(remote: Option<u64>) -> SteamSessionFacts {
        SteamSessionFacts {
            local: SteamId::from_raw(10),
            remote: remote.map(SteamId::from_raw),
        }
    }

    #[test]
    fn admission_waits_for_session_facts_and_link_then_registers_identity() {
        let mut app = App::new();
        app.add_plugins(SteamConnectionPlugin);
        let entity = app.world_mut().spawn((Link::default(), Linked)).id();
        app.world_mut().flush();
        assert!(app.world().get::<Connected>(entity).is_none());
        app.world_mut().entity_mut(entity).insert(facts(Some(20)));
        app.world_mut().flush();
        assert!(app.world().get::<Connected>(entity).is_some());
        assert_eq!(app.world().get::<RemoteId>(entity).unwrap().0, PeerId(20));
        assert_eq!(
            app.world()
                .resource::<NetworkingMetadata>()
                .peer_map
                .get(&PeerId(20)),
            Some(&entity)
        );
        app.world_mut().entity_mut(entity).insert(Unlinked {
            reason: UnlinkReason::UserRequested(None),
        });
        app.world_mut().flush();
        assert!(app.world().get::<Disconnected>(entity).is_some());
        assert!(
            !app.world()
                .resource::<NetworkingMetadata>()
                .peer_map
                .contains_key(&PeerId(20))
        );
    }

    #[cfg(feature = "client")]
    #[test]
    fn socket_session_uses_server_role_without_inventing_steam_identity() {
        let mut app = App::new();
        app.add_plugins(SteamConnectionPlugin);
        let entity = app
            .world_mut()
            .spawn((
                crate::client::SteamClientIo {
                    target: aeronet_steam::client::ConnectTarget::Addr(
                        "127.0.0.1:27015".parse().unwrap(),
                    ),
                    config: Default::default(),
                },
                facts(None),
            ))
            .id();
        app.world_mut().flush();
        assert!(app.world().get::<Connected>(entity).is_none());
        assert!(app.world().get::<RemoteId>(entity).is_none());
        app.world_mut().entity_mut(entity).insert(Linked);
        app.world_mut().flush();
        assert!(app.world().get::<Connected>(entity).is_some());
        assert_eq!(
            app.world()
                .resource::<NetworkingMetadata>()
                .peer_map
                .get(&PeerId::SERVER),
            Some(&entity)
        );
        assert_eq!(
            app.world().get::<RemoteId>(entity).unwrap().0,
            PeerId::SERVER
        );
        assert!(
            app.world()
                .get::<SteamSessionFacts>(entity)
                .unwrap()
                .remote
                .is_none()
        );
    }

    #[test]
    fn full_width_native_identity_is_preserved() {
        let mut app = App::new();
        app.add_plugins(SteamConnectionPlugin);
        let entity = app
            .world_mut()
            .spawn((Link::default(), Linked, facts(Some(u64::MAX))))
            .id();
        app.world_mut().flush();
        assert!(app.world().get::<Connected>(entity).is_some());
        assert_eq!(
            app.world().get::<RemoteId>(entity).unwrap().0,
            PeerId(u64::MAX)
        );
        assert_eq!(
            app.world()
                .get::<SteamSessionFacts>(entity)
                .unwrap()
                .remote
                .unwrap()
                .raw(),
            u64::MAX
        );
    }

    #[test]
    fn zero_native_identity_and_mismatched_expectations_are_rejected() {
        let mut app = App::new();
        app.add_plugins(SteamConnectionPlugin);
        let zero_remote = app
            .world_mut()
            .spawn((Link::default(), Linked, facts(Some(0))))
            .id();
        let zero_local = app
            .world_mut()
            .spawn((
                Link::default(),
                Linked,
                SteamSessionFacts {
                    local: SteamId::from_raw(0),
                    remote: Some(SteamId::from_raw(20)),
                },
            ))
            .id();
        let mismatch = app
            .world_mut()
            .spawn((
                Link::default(),
                Linked,
                facts(Some(30)),
                RemoteId(PeerId(40)),
            ))
            .id();
        app.world_mut().flush();
        for entity in [zero_remote, zero_local, mismatch] {
            assert!(app.world().get::<Connected>(entity).is_none());
            assert!(app.world().get::<Disconnected>(entity).is_some());
        }
        assert_eq!(app.world().get::<RemoteId>(mismatch).unwrap().0, PeerId(40));
    }

    #[test]
    fn address_target_cannot_authenticate_p2p_alias() {
        let mut app = App::new();
        app.add_plugins(SteamConnectionPlugin);
        let entity = app
            .world_mut()
            .spawn((
                Link::default(),
                Linked,
                facts(None),
                lightyear_connection::p2p::P2P::Inactive,
                RemoteId(PeerId(20)),
            ))
            .id();
        app.world_mut().flush();
        assert!(app.world().get::<Connected>(entity).is_none());
        assert!(app.world().get::<Disconnected>(entity).is_some());
        assert_eq!(app.world().get::<RemoteId>(entity).unwrap().0, PeerId(20));
    }

    #[test]
    fn transport_alone_does_not_admit_native_facts() {
        let mut app = App::new();
        app.add_plugins(crate::SteamTransportPlugin);
        let entity = app
            .world_mut()
            .spawn((Link::default(), facts(Some(20)), Linked))
            .id();
        app.world_mut().flush();
        assert!(app.world().get::<Connected>(entity).is_none());
        assert!(app.world().get::<LocalId>(entity).is_none());
        assert!(app.world().get::<RemoteId>(entity).is_none());
    }

    #[test]
    fn duplicate_native_identity_never_replaces_existing_connection() {
        let mut app = App::new();
        app.add_plugins(SteamConnectionPlugin);
        let first = app
            .world_mut()
            .spawn((Link::default(), Linked, facts(Some(20))))
            .id();
        let duplicate = app
            .world_mut()
            .spawn((Link::default(), Linked, facts(Some(20))))
            .id();
        app.world_mut().flush();
        assert!(app.world().get::<Connected>(first).is_some());
        assert!(app.world().get::<Connected>(duplicate).is_none());
        assert!(app.world().get::<Disconnected>(duplicate).is_some());
        assert_eq!(
            app.world()
                .resource::<NetworkingMetadata>()
                .peer_map
                .get(&PeerId(20)),
            Some(&first)
        );
    }
}
