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
/// This is a derived cache maintained by [`crate::ConnectionPlugin`]. Entities are only included
/// after their relevant lifecycle has completed: clients and P2P links must be [`Connected`],
/// and servers must be [`Started`].
#[derive(Resource, Default, Debug, Clone, PartialEq, Eq)]
pub enum AppMode {
    /// No standard Lightyear networking role is currently ready.
    #[default]
    Custom,
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

/// Marks the cached [`AppMode`] as needing to be recomputed.
#[derive(Resource, Default)]
struct AppModeDirty;

#[derive(Clone, Copy, Debug)]
struct ReadyClient {
    entity: Entity,
    is_p2p: bool,
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
        app.init_resource::<AppMode>();
        // Start dirty so that entities spawned before this plugin are classified on the first
        // update too.
        app.init_resource::<AppModeDirty>();

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

fn mark_dirty_on_insert(_trigger: On<Insert, ModeComponents>, mut commands: Commands) {
    commands.init_resource::<AppModeDirty>();
}

fn mark_dirty_on_remove(_trigger: On<Remove, ModeComponents>, mut commands: Commands) {
    commands.init_resource::<AppModeDirty>();
}

fn mark_dirty_on_discard(_trigger: On<Discard, ModeComponents>, mut commands: Commands) {
    commands.init_resource::<AppModeDirty>();
}

fn app_mode_is_dirty(dirty: Option<Res<AppModeDirty>>) -> bool {
    dirty.is_some()
}

fn refresh_app_mode(
    mut app_mode: ResMut<AppMode>,
    p2p_markers: Query<(), With<P2P>>,
    ready_clients: Query<
        (Entity, Has<P2P>, Has<HostClient>, Option<&LinkOf>),
        (With<Client>, With<Connected>),
    >,
    ready_servers: Query<Entity, (With<Server>, With<Started>)>,
    malformed_hosts: Query<Entity, (With<HostClient>, With<Connected>, Without<Client>)>,
    mut commands: Commands,
) {
    let clients = ready_clients
        .iter()
        .map(|(entity, is_p2p, is_host, link_of)| ReadyClient {
            entity,
            is_p2p,
            is_host,
            server: link_of.map(|link| link.server),
        })
        .collect();
    let servers = ready_servers.iter().collect();
    let malformed_hosts = malformed_hosts.iter().collect();

    let next = infer_app_mode(!p2p_markers.is_empty(), clients, servers, malformed_hosts);
    if *app_mode != next {
        if let AppMode::Invalid(error) = &next {
            tracing::error!(%error, "invalid Lightyear application mode");
        }
        *app_mode = next;
    }
    commands.remove_resource::<AppModeDirty>();
}

fn infer_app_mode(
    has_p2p_markers: bool,
    mut clients: SmallVec<[ReadyClient; 4]>,
    mut servers: SmallVec<[Entity; 4]>,
    mut malformed_hosts: SmallVec<[Entity; 1]>,
) -> AppMode {
    clients.sort_unstable_by_key(|client| client.entity.index_u32());
    servers.sort_unstable_by_key(|entity| entity.index_u32());
    malformed_hosts.sort_unstable_by_key(|entity| entity.index_u32());

    if has_p2p_markers {
        return AppMode::P2P(
            clients
                .iter()
                .filter(|client| client.is_p2p)
                .map(|client| client.entity)
                .collect(),
        );
    }

    if let Some(&client) = malformed_hosts.first() {
        return AppMode::Invalid(AppModeError::HostClientWithoutClient { client });
    }

    let conventional_clients: SmallVec<[ReadyClient; 4]> = clients
        .into_iter()
        .filter(|client| !client.is_p2p)
        .collect();

    if conventional_clients.len() > 1 {
        return AppMode::Invalid(AppModeError::MultipleConnectedClients(
            conventional_clients
                .iter()
                .map(|client| client.entity)
                .collect(),
        ));
    }
    if servers.len() > 1 {
        return AppMode::Invalid(AppModeError::MultipleStartedServers(servers));
    }

    match (conventional_clients.first(), servers.first().copied()) {
        (None, None) => AppMode::Custom,
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

    #[test]
    fn only_ready_client_and_server_entities_are_cached() {
        let mut app = test_app();
        let client = app.world_mut().spawn(Client).id();
        app.update();
        assert_eq!(*app.world().resource::<AppMode>(), AppMode::Custom);

        app.world_mut()
            .entity_mut(client)
            .insert((RemoteId(PeerId::Local(1)), Connected));
        app.update();
        assert_eq!(*app.world().resource::<AppMode>(), AppMode::Client(client));

        app.world_mut().entity_mut(client).insert(Disconnected {
            reason: Some("test".into()),
        });
        app.update();
        assert_eq!(*app.world().resource::<AppMode>(), AppMode::Custom);

        let server = app.world_mut().spawn(Server::default()).id();
        app.update();
        assert_eq!(*app.world().resource::<AppMode>(), AppMode::Custom);

        app.world_mut().entity_mut(server).insert(Started);
        app.update();
        assert_eq!(*app.world().resource::<AppMode>(), AppMode::Server(server));

        app.world_mut().entity_mut(server).insert(Stopped);
        app.update();
        assert_eq!(*app.world().resource::<AppMode>(), AppMode::Custom);
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
            *app.world().resource::<AppMode>(),
            AppMode::Invalid(AppModeError::HostClientServerNotStarted { client, server })
        );

        app.world_mut().entity_mut(server).insert(Started);
        app.update();
        assert_eq!(
            *app.world().resource::<AppMode>(),
            AppMode::HostClient { server, client }
        );
    }

    #[test]
    fn p2p_mode_exists_before_any_peer_is_connected_and_sorts_ready_links() {
        let mut app = test_app();
        let first = app.world_mut().spawn(P2P).id();
        let second = app.world_mut().spawn(P2P).id();

        app.update();
        assert_eq!(
            *app.world().resource::<AppMode>(),
            AppMode::P2P(SmallVec::new())
        );

        // Connect in reverse order to prove that insertion order does not affect the cache.
        app.world_mut()
            .entity_mut(second)
            .insert((RemoteId(PeerId::Local(2)), Connected));
        app.world_mut()
            .entity_mut(first)
            .insert((RemoteId(PeerId::Local(1)), Connected));
        app.update();

        assert_eq!(
            *app.world().resource::<AppMode>(),
            AppMode::P2P(SmallVec::from_slice(&[first, second]))
        );

        app.world_mut().entity_mut(first).insert(Disconnected {
            reason: Some("test".into()),
        });
        app.update();
        assert_eq!(
            *app.world().resource::<AppMode>(),
            AppMode::P2P(SmallVec::from_slice(&[second]))
        );

        app.world_mut().despawn(second);
        app.update();
        assert_eq!(
            *app.world().resource::<AppMode>(),
            AppMode::P2P(SmallVec::new())
        );

        app.world_mut().entity_mut(first).remove::<P2P>();
        app.update();
        assert_eq!(*app.world().resource::<AppMode>(), AppMode::Custom);
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
        assert_eq!(
            *app.world().resource::<AppMode>(),
            AppMode::P2P(SmallVec::from_slice(&[peer]))
        );
    }

    #[test]
    fn unsupported_ready_role_combinations_are_invalid() {
        let mut app = test_app();
        let first = connect_client(&mut app, 1);
        let second = connect_client(&mut app, 2);
        app.update();
        assert_eq!(
            *app.world().resource::<AppMode>(),
            AppMode::Invalid(AppModeError::MultipleConnectedClients(
                SmallVec::from_slice(&[first, second])
            ))
        );

        app.world_mut().despawn(second);
        let server = start_server(&mut app);
        app.update();
        assert_eq!(
            *app.world().resource::<AppMode>(),
            AppMode::Invalid(AppModeError::MixedClientServer {
                client: first,
                server,
            })
        );

        let other_server = start_server(&mut app);
        app.world_mut().despawn(first);
        app.update();
        assert_eq!(
            *app.world().resource::<AppMode>(),
            AppMode::Invalid(AppModeError::MultipleStartedServers(SmallVec::from_slice(
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
            *app.world().resource::<AppMode>(),
            AppMode::Invalid(AppModeError::HostClientMissingLinkOf { client })
        );
    }

    #[test]
    fn unchanged_recomputation_does_not_mark_app_mode_changed() {
        let mut app = test_app();
        let last_changed = app.world().resource_ref::<AppMode>().last_changed();

        app.world_mut().insert_resource(AppModeDirty);
        app.update();

        assert_eq!(
            app.world().resource_ref::<AppMode>().last_changed(),
            last_changed
        );
    }
}
