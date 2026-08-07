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
pub enum NetworkTopology {
    /// No networking topology has been identified yet.
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
    /// The declared and currently connected direct peer links in this P2P session.
    P2P {
        /// Connected P2P Link entities, sorted by local [`Entity`] ID.
        connected: SmallVec<[Entity; 4]>,
        /// Total number of Link entities carrying the [`P2P`] marker, including disconnected
        /// Links. This lets consumers test startup readiness without rediscovering the roster.
        declared_links: u8,
    },
    /// The ready entities do not form one supported networking topology.
    Invalid(NetworkTopologyError),
}

impl NetworkTopology {
    /// Returns true for a connected conventional client.
    pub fn is_client(&self) -> bool {
        matches!(self, Self::Client(_))
    }

    /// Returns true for a started server, including the server side of a host-client app.
    pub fn is_server(&self) -> bool {
        matches!(self, Self::Server(_) | Self::HostClient { .. })
    }

    /// Returns true for a started server without a connected in-process client.
    pub fn is_headless_server(&self) -> bool {
        matches!(self, Self::Server(_))
    }

    /// Returns true for a ready in-process host-client app.
    pub fn is_host_server(&self) -> bool {
        matches!(self, Self::HostClient { .. })
    }

    /// Returns true when direct P2P Links have been declared.
    pub fn is_p2p(&self) -> bool {
        matches!(self, Self::P2P { .. })
    }
}

/// Cached metadata describing the networking configuration of this Bevy application.
///
/// [`crate::ConnectionPlugin`] maintains this resource from role and lifecycle components. Users
/// can read [`mode`](Self::mode), but do not need to update it themselves.
#[derive(Resource, Debug, Clone)]
pub struct NetworkingMetadata {
    /// The currently identified networking topology.
    pub mode: NetworkTopology,
    // This is kept in the same resource as the mode, but mutated without triggering Bevy change
    // detection. Consumers therefore only observe a change after `mode` itself changes.
    dirty: bool,
}

impl Default for NetworkingMetadata {
    fn default() -> Self {
        Self {
            mode: NetworkTopology::Undefined,
            dirty: true,
        }
    }
}

/// Why ready networking entities could not be classified into a supported [`NetworkTopology`].
#[derive(thiserror::Error, Debug, Clone, PartialEq, Eq)]
pub enum NetworkTopologyError {
    /// Ready P2P and conventional networking roles exist in the same application.
    #[error(
        "P2P link {p2p:?} is ready alongside conventional roles (client: {conventional_client:?}, server: {server:?})"
    )]
    MixedP2PAndConventional {
        /// One of the application's declared P2P Links.
        p2p: Entity,
        /// A connected conventional Client or HostClient, when present.
        conventional_client: Option<Entity>,
        /// A started conventional Server, when present.
        server: Option<Entity>,
    },
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

/// System set that refreshes the cached [`NetworkTopology`].
#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone, Copy)]
pub enum NetworkTopologySystems {
    /// Infer the topology after networking lifecycle changes.
    Update,
}

#[derive(Clone, Copy, Debug)]
struct ReadyClient {
    entity: Entity,
    is_host: bool,
    server: Option<Entity>,
}

type TopologyComponents = (
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

pub(crate) struct NetworkTopologyPlugin;

impl Plugin for NetworkTopologyPlugin {
    fn build(&self, app: &mut App) {
        // NetworkingMetadata starts dirty so that entities spawned before this plugin are
        // classified on the first update too.
        app.init_resource::<NetworkingMetadata>();

        app.add_observer(mark_dirty_on_insert);
        app.add_observer(mark_dirty_on_remove);
        app.add_observer(mark_dirty_on_discard);

        app.configure_sets(
            PreUpdate,
            NetworkTopologySystems::Update
                .after(LinkSystems::Receive)
                .after(ConnectionSystems::Receive),
        );
        app.add_systems(
            PreUpdate,
            refresh_network_topology
                .in_set(NetworkTopologySystems::Update)
                .run_if(network_topology_is_dirty),
        );

        app.configure_sets(
            PostUpdate,
            NetworkTopologySystems::Update.before(ConnectionSystems::Send),
        );
        app.add_systems(
            PostUpdate,
            refresh_network_topology
                .in_set(NetworkTopologySystems::Update)
                .run_if(network_topology_is_dirty),
        );
    }
}

fn mark_dirty_on_insert(
    _trigger: On<Insert, TopologyComponents>,
    mut metadata: ResMut<NetworkingMetadata>,
) {
    metadata.bypass_change_detection().dirty = true;
}

fn mark_dirty_on_remove(
    _trigger: On<Remove, TopologyComponents>,
    mut metadata: ResMut<NetworkingMetadata>,
) {
    metadata.bypass_change_detection().dirty = true;
}

fn mark_dirty_on_discard(
    _trigger: On<Discard, TopologyComponents>,
    mut metadata: ResMut<NetworkingMetadata>,
) {
    metadata.bypass_change_detection().dirty = true;
}

fn network_topology_is_dirty(metadata: Res<NetworkingMetadata>) -> bool {
    metadata.dirty
}

fn refresh_network_topology(
    mut metadata: ResMut<NetworkingMetadata>,
    p2p_markers: Query<Entity, With<P2P>>,
    ready_clients: Query<
        (Entity, Has<P2P>, Has<HostClient>, Option<&LinkOf>),
        (With<Client>, With<Connected>),
    >,
    ready_servers: Query<Entity, (With<Server>, With<Started>)>,
    malformed_hosts: Query<Entity, (With<HostClient>, With<Connected>, Without<Client>)>,
) {
    let malformed_host = malformed_hosts
        .iter()
        .min_by_key(|entity| entity.index_u32());
    let first_p2p = p2p_markers.iter().min_by_key(|entity| entity.index_u32());
    let next = if let Some(client) = malformed_host {
        NetworkTopology::Invalid(NetworkTopologyError::HostClientWithoutClient { client })
    } else if let Some(p2p) = first_p2p {
        // A connected non-P2P Client is conventional. HostClient is conventional even if it was
        // accidentally combined with P2P on the same Link.
        let conventional_client = ready_clients
            .iter()
            .filter(|(_, is_p2p, is_host, _)| !*is_p2p || *is_host)
            .map(|(entity, _, _, _)| entity)
            .min_by_key(|entity| entity.index_u32());
        let server = ready_servers.iter().min_by_key(|entity| entity.index_u32());
        if conventional_client.is_some() || server.is_some() {
            NetworkTopology::Invalid(NetworkTopologyError::MixedP2PAndConventional {
                p2p,
                conventional_client,
                server,
            })
        } else {
            let declared_links = u8::try_from(p2p_markers.iter().count()).unwrap_or_else(|_| {
                tracing::error!(
                    maximum = u8::MAX,
                    "P2P topology declared more Links than its cached count can represent"
                );
                u8::MAX
            });
            let mut connected: SmallVec<[Entity; 4]> = ready_clients
                .iter()
                .filter(|(_, is_p2p, _, _)| *is_p2p)
                .map(|(entity, _, _, _)| entity)
                .collect();
            connected.sort_unstable_by_key(|entity| entity.index_u32());
            NetworkTopology::P2P {
                connected,
                declared_links,
            }
        }
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
            Err(error) => NetworkTopology::Invalid(error),
            Ok(client) => match unique_ready_server(ready_servers.iter()) {
                Err(error) => NetworkTopology::Invalid(error),
                Ok(server) => infer_standard_topology(client, server),
            },
        }
    };

    let mode_changed = metadata.mode != next;
    metadata.bypass_change_detection().dirty = false;
    if mode_changed {
        if let NetworkTopology::Invalid(error) = &next {
            tracing::error!(%error, "invalid Lightyear networking topology");
        }
        metadata.mode = next;
    }
}

fn unique_ready_client(
    mut clients: impl Iterator<Item = ReadyClient>,
) -> Result<Option<ReadyClient>, NetworkTopologyError> {
    let Some(first) = clients.next() else {
        return Ok(None);
    };
    let Some(second) = clients.next() else {
        return Ok(Some(first));
    };

    let mut entities: SmallVec<[Entity; 4]> = SmallVec::from_slice(&[first.entity, second.entity]);
    entities.extend(clients.map(|client| client.entity));
    entities.sort_unstable_by_key(|entity| entity.index_u32());
    Err(NetworkTopologyError::MultipleConnectedClients(entities))
}

fn unique_ready_server(
    mut servers: impl Iterator<Item = Entity>,
) -> Result<Option<Entity>, NetworkTopologyError> {
    let Some(first) = servers.next() else {
        return Ok(None);
    };
    let Some(second) = servers.next() else {
        return Ok(Some(first));
    };

    let mut entities: SmallVec<[Entity; 4]> = SmallVec::from_slice(&[first, second]);
    entities.extend(servers);
    entities.sort_unstable_by_key(|entity| entity.index_u32());
    Err(NetworkTopologyError::MultipleStartedServers(entities))
}

fn infer_standard_topology(client: Option<ReadyClient>, server: Option<Entity>) -> NetworkTopology {
    match (client.as_ref(), server) {
        (None, None) => NetworkTopology::Undefined,
        (None, Some(server)) => NetworkTopology::Server(server),
        (Some(client), None) if client.is_host => match client.server {
            Some(server) => {
                NetworkTopology::Invalid(NetworkTopologyError::HostClientServerNotStarted {
                    client: client.entity,
                    server,
                })
            }
            None => NetworkTopology::Invalid(NetworkTopologyError::HostClientMissingLinkOf {
                client: client.entity,
            }),
        },
        (Some(client), None) => NetworkTopology::Client(client.entity),
        (Some(client), Some(server)) if client.is_host => match client.server {
            Some(linked_server) if linked_server == server => NetworkTopology::HostClient {
                server,
                client: client.entity,
            },
            Some(linked_server) => {
                NetworkTopology::Invalid(NetworkTopologyError::HostClientServerNotStarted {
                    client: client.entity,
                    server: linked_server,
                })
            }
            None => NetworkTopology::Invalid(NetworkTopologyError::HostClientMissingLinkOf {
                client: client.entity,
            }),
        },
        (Some(client), Some(server)) => {
            NetworkTopology::Invalid(NetworkTopologyError::MixedClientServer {
                client: client.entity,
                server,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::DisconnectedReason;
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

    fn mode(app: &App) -> &NetworkTopology {
        &app.world().resource::<NetworkingMetadata>().mode
    }

    #[test]
    fn only_ready_client_and_server_entities_are_cached() {
        let mut app = test_app();
        let client = app.world_mut().spawn(Client).id();
        app.update();
        assert_eq!(mode(&app), &NetworkTopology::Undefined);

        app.world_mut()
            .entity_mut(client)
            .insert((RemoteId(PeerId::Local(1)), Connected));
        app.update();
        assert_eq!(mode(&app), &NetworkTopology::Client(client));

        app.world_mut().entity_mut(client).insert(Disconnected {
            reason: DisconnectedReason::UserRequested(Some("test".into())),
        });
        app.update();
        assert_eq!(mode(&app), &NetworkTopology::Undefined);

        let server = app.world_mut().spawn(Server::default()).id();
        app.update();
        assert_eq!(mode(&app), &NetworkTopology::Undefined);

        app.world_mut().entity_mut(server).insert(Started);
        app.update();
        assert_eq!(mode(&app), &NetworkTopology::Server(server));

        app.world_mut().entity_mut(server).insert(Stopped);
        app.update();
        assert_eq!(mode(&app), &NetworkTopology::Undefined);
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
            &NetworkTopology::Invalid(NetworkTopologyError::HostClientServerNotStarted {
                client,
                server
            })
        );

        app.world_mut().entity_mut(server).insert(Started);
        app.update();
        assert_eq!(mode(&app), &NetworkTopology::HostClient { server, client });
    }

    #[test]
    fn p2p_mode_exists_before_any_peer_is_connected_and_sorts_ready_links() {
        let mut app = test_app();
        let first = app.world_mut().spawn(P2P).id();
        let second = app.world_mut().spawn(P2P).id();

        app.update();
        assert_eq!(
            mode(&app),
            &NetworkTopology::P2P {
                connected: SmallVec::new(),
                declared_links: 2,
            }
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
            mode(&app),
            &NetworkTopology::P2P {
                connected: SmallVec::from_slice(&[first, second]),
                declared_links: 2,
            }
        );

        app.world_mut().entity_mut(first).insert(Disconnected {
            reason: DisconnectedReason::UserRequested(Some("test".into())),
        });
        app.update();
        assert_eq!(
            mode(&app),
            &NetworkTopology::P2P {
                connected: SmallVec::from_slice(&[second]),
                declared_links: 2,
            }
        );

        app.world_mut().despawn(second);
        app.update();
        assert_eq!(
            mode(&app),
            &NetworkTopology::P2P {
                connected: SmallVec::new(),
                declared_links: 1,
            }
        );

        app.world_mut().entity_mut(first).remove::<P2P>();
        app.update();
        assert_eq!(mode(&app), &NetworkTopology::Undefined);
    }

    #[test]
    fn ready_p2p_and_conventional_roles_are_invalid() {
        let mut app = test_app();
        let peer = app
            .world_mut()
            .spawn((P2P, RemoteId(PeerId::Local(1)), Connected))
            .id();
        let client = connect_client(&mut app, 2);
        let server = start_server(&mut app);

        app.update();
        assert_eq!(
            mode(&app),
            &NetworkTopology::Invalid(NetworkTopologyError::MixedP2PAndConventional {
                p2p: peer,
                conventional_client: Some(client),
                server: Some(server),
            })
        );
    }

    #[test]
    fn unready_conventional_roles_do_not_conflict_with_p2p() {
        let mut app = test_app();
        let peer = app
            .world_mut()
            .spawn((P2P, RemoteId(PeerId::Local(1)), Connected))
            .id();
        app.world_mut().spawn(Client);
        app.world_mut().spawn(Server::default());

        app.update();
        assert_eq!(
            mode(&app),
            &NetworkTopology::P2P {
                connected: SmallVec::from_slice(&[peer]),
                declared_links: 1,
            }
        );
    }

    #[test]
    fn unsupported_ready_role_combinations_are_invalid() {
        let mut app = test_app();
        let first = connect_client(&mut app, 1);
        let second = connect_client(&mut app, 2);
        app.update();
        assert_eq!(
            mode(&app),
            &NetworkTopology::Invalid(NetworkTopologyError::MultipleConnectedClients(
                SmallVec::from_slice(&[first, second])
            ))
        );

        app.world_mut().despawn(second);
        let server = start_server(&mut app);
        app.update();
        assert_eq!(
            mode(&app),
            &NetworkTopology::Invalid(NetworkTopologyError::MixedClientServer {
                client: first,
                server,
            })
        );

        let other_server = start_server(&mut app);
        app.world_mut().despawn(first);
        app.update();
        assert_eq!(
            mode(&app),
            &NetworkTopology::Invalid(NetworkTopologyError::MultipleStartedServers(
                SmallVec::from_slice(&[server, other_server])
            ))
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
            &NetworkTopology::Invalid(NetworkTopologyError::HostClientMissingLinkOf { client })
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
