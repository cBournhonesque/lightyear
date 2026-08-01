use crate::ConnectionSystems;
use crate::client::{Client, Connected, Disconnected};
use crate::host::HostClient;
use crate::p2p::P2P;
use crate::server::{Started, Stopped};
use bevy_app::{App, Plugin, PostUpdate, PreUpdate};
use bevy_ecs::prelude::*;
use lightyear_link::LinkSystems;
use lightyear_link::prelude::{LinkOf, Server};
use smallvec::SmallVec;

/// The ready networking role of this Bevy application.
///
/// Entities are only included after their relevant lifecycle has completed: clients and P2P links
/// must be [`Connected`], and servers must be [`Started`].
#[derive(Default, Debug, Clone, PartialEq, Eq)]
pub enum AppMode {
    /// No networking mode has been identified yet.
    #[default]
    Undefined,
    /// A connected client and its server link.
    Client(Entity),
    /// A started server.
    Server(Entity),
    /// A connected in-process client and its started server.
    HostClient {
        /// The started server entity.
        server: Entity,
        /// The connected host-client link entity.
        client: Entity,
    },
    /// The connected direct peer links in this P2P session.
    ///
    /// This can be empty while the session is waiting for its first peer to connect.
    P2P(SmallVec<[Entity; 4]>),
    /// The ready entities do not form one supported application role.
    Invalid(AppModeError),
}

/// Cached metadata describing the networking configuration of this Bevy application.
///
/// [`crate::ConnectionPlugin`] maintains this resource from role and lifecycle components. Users
/// can read [`mode`](Self::mode), but do not need to update it themselves.
#[derive(Resource, Debug, Clone)]
pub struct NetworkingMetadata {
    /// The currently identified networking mode.
    pub mode: AppMode,
    // This is kept in the same resource as the mode, but mutated without triggering Bevy change
    // detection. Consumers therefore only observe a change after `mode` itself changes.
    dirty: bool,
}

impl Default for NetworkingMetadata {
    fn default() -> Self {
        Self {
            mode: AppMode::Undefined,
            dirty: true,
        }
    }
}

/// Why ready networking entities could not be classified into a supported [`AppMode`].
#[derive(thiserror::Error, Debug, Clone, PartialEq, Eq)]
pub enum AppModeError {
    /// More than one conventional client link is connected.
    #[error("multiple conventional client links are connected: {0:?}")]
    MultipleConnectedClients(SmallVec<[Entity; 4]>),
    /// More than one server is started.
    #[error("multiple servers are started: {0:?}")]
    MultipleStartedServers(SmallVec<[Entity; 4]>),
    /// A connected host-client does not have a `Client` marker.
    #[error("connected host-client {client:?} does not have a Client marker")]
    HostClientWithoutClient {
        /// The malformed host-client entity.
        client: Entity,
    },
    /// A connected host-client does not identify its in-process server.
    #[error("connected host-client {client:?} does not have a LinkOf relationship")]
    HostClientMissingLinkOf {
        /// The malformed host-client entity.
        client: Entity,
    },
    /// The server referenced by a connected host-client is not started.
    #[error(
        "connected host-client {client:?} references server {server:?}, but that server is not started"
    )]
    HostClientServerNotStarted {
        /// The connected host-client entity.
        client: Entity,
        /// The referenced server entity.
        server: Entity,
    },
    /// A conventional client and server are both ready but do not form a host-client pair.
    #[error(
        "connected client {client:?} and started server {server:?} do not form a host-client pair"
    )]
    MixedClientServer {
        /// The connected client entity.
        client: Entity,
        /// The started server entity.
        server: Entity,
    },
}

/// System set that refreshes the cached [`AppMode`].
#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone, Copy)]
pub enum AppModeSystems {
    /// Infer the application mode after networking lifecycle changes.
    Update,
}

#[derive(Clone, Copy, Debug)]
struct ReadyClient {
    entity: Entity,
    is_host: bool,
    server: Option<Entity>,
}

type ModeComponents = (
    Client,
    Server,
    HostClient,
    LinkOf,
    P2P,
    Connected,
    Disconnected,
    Started,
    Stopped,
);

pub(crate) struct AppModePlugin;

impl Plugin for AppModePlugin {
    fn build(&self, app: &mut App) {
        // NetworkingMetadata starts dirty so that entities spawned before this plugin are
        // classified on the first update too.
        app.init_resource::<NetworkingMetadata>();

        app.add_observer(mark_dirty_on_insert);
        app.add_observer(mark_dirty_on_remove);
        app.add_observer(mark_dirty_on_discard);

        app.configure_sets(
            PreUpdate,
            AppModeSystems::Update
                .after(LinkSystems::Receive)
                .after(ConnectionSystems::Receive),
        );
        app.add_systems(
            PreUpdate,
            refresh_app_mode
                .in_set(AppModeSystems::Update)
                .run_if(app_mode_is_dirty),
        );

        app.configure_sets(
            PostUpdate,
            AppModeSystems::Update.before(ConnectionSystems::Send),
        );
        app.add_systems(
            PostUpdate,
            refresh_app_mode
                .in_set(AppModeSystems::Update)
                .run_if(app_mode_is_dirty),
        );
    }
}

fn mark_dirty_on_insert(
    _trigger: On<Insert, ModeComponents>,
    mut metadata: ResMut<NetworkingMetadata>,
) {
    metadata.bypass_change_detection().dirty = true;
}

fn mark_dirty_on_remove(
    _trigger: On<Remove, ModeComponents>,
    mut metadata: ResMut<NetworkingMetadata>,
) {
    metadata.bypass_change_detection().dirty = true;
}

fn mark_dirty_on_discard(
    _trigger: On<Discard, ModeComponents>,
    mut metadata: ResMut<NetworkingMetadata>,
) {
    metadata.bypass_change_detection().dirty = true;
}

fn app_mode_is_dirty(metadata: Res<NetworkingMetadata>) -> bool {
    metadata.dirty
}

fn refresh_app_mode(
    mut metadata: ResMut<NetworkingMetadata>,
    p2p_markers: Query<(), With<P2P>>,
    ready_clients: Query<
        (Entity, Has<P2P>, Has<HostClient>, Option<&LinkOf>),
        (With<Client>, With<Connected>),
    >,
    ready_servers: Query<Entity, (With<Server>, With<Started>)>,
    malformed_hosts: Query<Entity, (With<HostClient>, With<Connected>, Without<Client>)>,
) {
    let next = if !p2p_markers.is_empty() {
        let mut links: SmallVec<[Entity; 4]> = ready_clients
            .iter()
            .filter(|(_, is_p2p, _, _)| *is_p2p)
            .map(|(entity, _, _, _)| entity)
            .collect();
        links.sort_unstable_by_key(|entity| entity.index_u32());
        AppMode::P2P(links)
    } else if let Some(client) = malformed_hosts
        .iter()
        .min_by_key(|entity| entity.index_u32())
    {
        AppMode::Invalid(AppModeError::HostClientWithoutClient { client })
    } else {
        let client = unique_ready_client(ready_clients.iter().filter_map(
            |(entity, is_p2p, is_host, link_of)| {
                (!is_p2p).then_some(ReadyClient {
                    entity,
                    is_host,
                    server: link_of.map(|link| link.server),
                })
            },
        ));
        match client {
            Err(error) => AppMode::Invalid(error),
            Ok(client) => match unique_ready_server(ready_servers.iter()) {
                Err(error) => AppMode::Invalid(error),
                Ok(server) => infer_standard_mode(client, server),
            },
        }
    };

    let mode_changed = metadata.mode != next;
    metadata.bypass_change_detection().dirty = false;
    if mode_changed {
        if let AppMode::Invalid(error) = &next {
            tracing::error!(%error, "invalid Lightyear application mode");
        }
        metadata.mode = next;
    }
}

fn unique_ready_client(
    mut clients: impl Iterator<Item = ReadyClient>,
) -> Result<Option<ReadyClient>, AppModeError> {
    let Some(first) = clients.next() else {
        return Ok(None);
    };
    let Some(second) = clients.next() else {
        return Ok(Some(first));
    };

    let mut entities: SmallVec<[Entity; 4]> = SmallVec::from_slice(&[first.entity, second.entity]);
    entities.extend(clients.map(|client| client.entity));
    entities.sort_unstable_by_key(|entity| entity.index_u32());
    Err(AppModeError::MultipleConnectedClients(entities))
}

fn unique_ready_server(
    mut servers: impl Iterator<Item = Entity>,
) -> Result<Option<Entity>, AppModeError> {
    let Some(first) = servers.next() else {
        return Ok(None);
    };
    let Some(second) = servers.next() else {
        return Ok(Some(first));
    };

    let mut entities: SmallVec<[Entity; 4]> = SmallVec::from_slice(&[first, second]);
    entities.extend(servers);
    entities.sort_unstable_by_key(|entity| entity.index_u32());
    Err(AppModeError::MultipleStartedServers(entities))
}

fn infer_standard_mode(client: Option<ReadyClient>, server: Option<Entity>) -> AppMode {
    match (client.as_ref(), server) {
        (None, None) => AppMode::Undefined,
        (None, Some(server)) => AppMode::Server(server),
        (Some(client), None) if client.is_host => match client.server {
            Some(server) => AppMode::Invalid(AppModeError::HostClientServerNotStarted {
                client: client.entity,
                server,
            }),
            None => AppMode::Invalid(AppModeError::HostClientMissingLinkOf {
                client: client.entity,
            }),
        },
        (Some(client), None) => AppMode::Client(client.entity),
        (Some(client), Some(server)) if client.is_host => match client.server {
            Some(linked_server) if linked_server == server => AppMode::HostClient {
                server,
                client: client.entity,
            },
            Some(linked_server) => AppMode::Invalid(AppModeError::HostClientServerNotStarted {
                client: client.entity,
                server: linked_server,
            }),
            None => AppMode::Invalid(AppModeError::HostClientMissingLinkOf {
                client: client.entity,
            }),
        },
        (Some(client), Some(server)) => AppMode::Invalid(AppModeError::MixedClientServer {
            client: client.entity,
            server,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::PeerMetadata;
    use alloc::vec::Vec;
    use bevy_ecs::change_detection::DetectChanges;
    use lightyear_core::id::{PeerId, RemoteId};

    fn test_app() -> App {
        let mut app = App::new();
        // Connected and Started maintain this existing shared connection resource in their hooks.
        app.init_resource::<PeerMetadata>();
        app.add_plugins(crate::ConnectionPlugin);
        app.update();
        app
    }

    fn connect_client(app: &mut App, peer: u64) -> Entity {
        app.world_mut()
            .spawn((Client, RemoteId(PeerId::Local(peer)), Connected))
            .id()
    }

    fn start_server(app: &mut App) -> Entity {
        app.world_mut().spawn((Server::default(), Started)).id()
    }

    fn mode(app: &App) -> &AppMode {
        &app.world().resource::<NetworkingMetadata>().mode
    }

    #[test]
    fn only_ready_client_and_server_entities_are_cached() {
        let mut app = test_app();
        let client = app.world_mut().spawn(Client).id();
        app.update();
        assert_eq!(mode(&app), &AppMode::Undefined);

        app.world_mut()
            .entity_mut(client)
            .insert((RemoteId(PeerId::Local(1)), Connected));
        app.update();
        assert_eq!(mode(&app), &AppMode::Client(client));

        app.world_mut().entity_mut(client).insert(Disconnected {
            reason: Some("test".into()),
        });
        app.update();
        assert_eq!(mode(&app), &AppMode::Undefined);

        let server = app.world_mut().spawn(Server::default()).id();
        app.update();
        assert_eq!(mode(&app), &AppMode::Undefined);

        app.world_mut().entity_mut(server).insert(Started);
        app.update();
        assert_eq!(mode(&app), &AppMode::Server(server));

        app.world_mut().entity_mut(server).insert(Stopped);
        app.update();
        assert_eq!(mode(&app), &AppMode::Undefined);
    }

    #[test]
    fn host_client_requires_a_connected_client_and_its_started_server() {
        let mut app = test_app();
        let server = app.world_mut().spawn(Server::default()).id();
        let client = app
            .world_mut()
            .spawn((
                Client,
                RemoteId(PeerId::Local(0)),
                Connected,
                LinkOf { server },
                HostClient { buffer: Vec::new() },
            ))
            .id();

        app.update();
        assert_eq!(
            mode(&app),
            &AppMode::Invalid(AppModeError::HostClientServerNotStarted { client, server })
        );

        app.world_mut().entity_mut(server).insert(Started);
        app.update();
        assert_eq!(mode(&app), &AppMode::HostClient { server, client });
    }

    #[test]
    fn p2p_mode_exists_before_any_peer_is_connected_and_sorts_ready_links() {
        let mut app = test_app();
        let first = app.world_mut().spawn(P2P).id();
        let second = app.world_mut().spawn(P2P).id();

        app.update();
        assert_eq!(mode(&app), &AppMode::P2P(SmallVec::new()));

        // Connect in reverse order to prove that insertion order does not affect the cache.
        app.world_mut()
            .entity_mut(second)
            .insert((RemoteId(PeerId::Local(2)), Connected));
        app.world_mut()
            .entity_mut(first)
            .insert((RemoteId(PeerId::Local(1)), Connected));
        app.update();

        assert_eq!(
            mode(&app),
            &AppMode::P2P(SmallVec::from_slice(&[first, second]))
        );

        app.world_mut().entity_mut(first).insert(Disconnected {
            reason: Some("test".into()),
        });
        app.update();
        assert_eq!(mode(&app), &AppMode::P2P(SmallVec::from_slice(&[second])));

        app.world_mut().despawn(second);
        app.update();
        assert_eq!(mode(&app), &AppMode::P2P(SmallVec::new()));

        app.world_mut().entity_mut(first).remove::<P2P>();
        app.update();
        assert_eq!(mode(&app), &AppMode::Undefined);
    }

    #[test]
    fn p2p_markers_take_priority_over_conventional_roles() {
        let mut app = test_app();
        let peer = app
            .world_mut()
            .spawn((P2P, RemoteId(PeerId::Local(1)), Connected))
            .id();
        connect_client(&mut app, 2);
        start_server(&mut app);

        app.update();
        assert_eq!(mode(&app), &AppMode::P2P(SmallVec::from_slice(&[peer])));
    }

    #[test]
    fn unsupported_ready_role_combinations_are_invalid() {
        let mut app = test_app();
        let first = connect_client(&mut app, 1);
        let second = connect_client(&mut app, 2);
        app.update();
        assert_eq!(
            mode(&app),
            &AppMode::Invalid(AppModeError::MultipleConnectedClients(
                SmallVec::from_slice(&[first, second])
            ))
        );

        app.world_mut().despawn(second);
        let server = start_server(&mut app);
        app.update();
        assert_eq!(
            mode(&app),
            &AppMode::Invalid(AppModeError::MixedClientServer {
                client: first,
                server,
            })
        );

        let other_server = start_server(&mut app);
        app.world_mut().despawn(first);
        app.update();
        assert_eq!(
            mode(&app),
            &AppMode::Invalid(AppModeError::MultipleStartedServers(SmallVec::from_slice(
                &[server, other_server]
            )))
        );
    }

    #[test]
    fn connected_host_without_link_relationship_is_invalid() {
        let mut app = test_app();
        let client = app
            .world_mut()
            .spawn((
                Client,
                RemoteId(PeerId::Local(0)),
                Connected,
                HostClient { buffer: Vec::new() },
            ))
            .id();

        app.update();
        assert_eq!(
            mode(&app),
            &AppMode::Invalid(AppModeError::HostClientMissingLinkOf { client })
        );
    }

    #[test]
    fn unchanged_invalidation_does_not_mark_networking_metadata_changed() {
        let mut app = test_app();
        let last_changed = app
            .world()
            .resource_ref::<NetworkingMetadata>()
            .last_changed();

        // This invalidates the cache, but a disconnected Client does not change the mode.
        app.world_mut().spawn(Client);
        app.update();

        assert_eq!(
            app.world()
                .resource_ref::<NetworkingMetadata>()
                .last_changed(),
            last_changed
        );
    }
}
