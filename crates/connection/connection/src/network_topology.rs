use crate::ConnectionSystems;
use crate::client::{Client, Connected, Disconnected};
use crate::host::HostClient;
use crate::p2p::{P2P, P2PRoster, P2PSessionPhase};
use crate::server::{Started, Stopped};
use bevy_app::{App, Plugin, PostUpdate};
use bevy_ecs::prelude::*;
use bevy_platform::collections::HashMap;
use lightyear_core::id::PeerId;
use lightyear_link::prelude::{LinkOf, Server};
use smallvec::SmallVec;

/// The ready networking role of this Bevy application.
///
/// Conventional clients are included after they are [`Connected`] and servers after they are
/// [`Started`]. P2P is exposed only after its start barrier has completed. Systems that need to
/// inspect lobby or startup participation should query [`P2P`] components directly.
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
    /// A deterministic P2P session in progress on this application.
    ///
    /// Present for the whole session lifecycle: while the start barrier forms the cohort, while
    /// this application is a join candidate of a running session, and while it plays. Read
    /// [`P2PRoster::phase`] to tell those apart.
    ///
    /// The roster caches the session membership: only the [`P2P::Joined`] and connected Links are
    /// [`started`](P2PRoster::started), and peers that have been admitted but are not playing yet
    /// appear in the candidate sets. Consumers read this instead of re-querying the Links.
    ///
    /// [`is_p2p`](NetworkTopology::is_p2p) is deliberately narrower than the variant: it answers
    /// "is this application a started peer", which is the question gameplay machinery asks, and it
    /// is false while a cohort is still forming.
    P2P(P2PRoster),
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

    /// Whether this application is a **started peer** of a deterministic P2P session.
    ///
    /// This is false while a cohort is still forming and while this application is a join
    /// candidate, even though [`P2P`](Self::P2P) is already present in both cases. Gameplay
    /// machinery must key on this: the deterministic world does not exist before the session has
    /// started.
    pub fn is_p2p(&self) -> bool {
        matches!(
            self,
            Self::P2P(P2PRoster {
                phase: P2PSessionPhase::Active,
                ..
            })
        )
    }

    /// The P2P session on this application, whatever its phase.
    ///
    /// Unlike [`is_p2p`](Self::is_p2p), this is `Some` as soon as a cohort is being formed or this
    /// application is being admitted to a running session — which is exactly when the
    /// synchronization and input layers need the membership.
    pub fn p2p_roster(&self) -> Option<&P2PRoster> {
        match self {
            Self::P2P(roster) => Some(roster),
            _ => None,
        }
    }

    /// The P2P roster of this application once it is a started peer.
    pub fn started_p2p_roster(&self) -> Option<&P2PRoster> {
        self.p2p_roster()
            .filter(|roster| matches!(roster.phase, P2PSessionPhase::Active))
    }
}

/// Cached metadata describing the networking configuration of this Bevy application.
///
/// [`crate::ConnectionPlugin`] maintains this resource from role and lifecycle components. Users
/// can read [`mode`](Self::mode) and [`peer_map`](Self::peer_map), but do not need to
/// update them themselves.
#[derive(Resource, Debug, Clone)]
pub struct NetworkingMetadata {
    /// The currently identified networking topology.
    pub mode: NetworkTopology,
    /// Mapping from remote peer IDs to their local connection entities.
    pub peer_map: HashMap<PeerId, Entity>,
    // This is mutated without triggering Bevy change detection. Consumers therefore only observe
    // a change after `mode` itself changes.
    dirty: bool,
}

impl Default for NetworkingMetadata {
    fn default() -> Self {
        Self {
            mode: NetworkTopology::Undefined,
            peer_map: HashMap::default(),
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

        // Reconcile ordinary lifecycle changes once per frame. The P2P start barrier updates the
        // topology immediately when it activates a session, before its first fixed gameplay tick.
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

fn network_topology_is_dirty(
    metadata: Res<NetworkingMetadata>,
    phase: Res<P2PSessionPhase>,
) -> bool {
    metadata.dirty || phase.is_changed()
}

fn refresh_network_topology(
    mut metadata: ResMut<NetworkingMetadata>,
    phase: Res<P2PSessionPhase>,
    p2p_links: Query<(Entity, &P2P, Has<Connected>)>,
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
    let roster = P2PRoster::from_links(
        *phase,
        p2p_links
            .iter()
            .map(|(entity, state, connected)| (entity, *state, connected)),
    );
    let first_active_p2p = p2p_links
        .iter()
        .filter_map(|(entity, state, _)| (*state != P2P::Inactive).then_some(entity))
        .min_by_key(|entity| entity.index_u32());

    // A connected non-P2P Client is conventional. HostClient is conventional even if it was
    // accidentally combined with P2P on the same Link.
    let conventional_client = ready_clients
        .iter()
        .filter(|(_, is_p2p, is_host, _)| !*is_p2p || *is_host)
        .map(|(entity, _, _, _)| entity)
        .min_by_key(|entity| entity.index_u32());
    let server = ready_servers.iter().min_by_key(|entity| entity.index_u32());
    let mixed_roles = if let Some(p2p) = first_active_p2p
        && (conventional_client.is_some() || server.is_some())
    {
        Some(NetworkTopologyError::MixedP2PAndConventional {
            p2p,
            conventional_client,
            server,
        })
    } else {
        None
    };

    let next = if let Some(client) = malformed_host {
        NetworkTopology::Invalid(NetworkTopologyError::HostClientWithoutClient { client })
    } else if let Some(error) = mixed_roles {
        NetworkTopology::Invalid(error)
    } else if !matches!(*phase, P2PSessionPhase::Stopped) {
        // The session owns its roster for as long as it exists, whatever its phase: a forming
        // cohort, a peer being admitted, and a started session all need the membership exposed. It
        // is only the started-peer predicates that are narrower. A session with no declared
        // non-Inactive P2P Link is a solo one: the local peer plays alone and may admit a joiner
        // later.
        NetworkTopology::P2P(roster)
    } else if first_active_p2p.is_some() {
        // Link membership cannot activate a stopped session.
        NetworkTopology::Undefined
    } else {
        let client = unique_ready_client(ready_clients.iter().filter_map(
            |(entity, is_p2p, is_host, link_of)| {
                (!is_p2p).then_some(ReadyClient {
                    entity,
                    is_host,
                    server: link_of.map(|link| link.endpoint),
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
    use alloc::vec::Vec;
    use bevy_ecs::change_detection::DetectChanges;
    use lightyear_core::id::{PeerId, RemoteId};

    fn test_app() -> App {
        let mut app = App::new();
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
                LinkOf { endpoint: server },
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
    fn p2p_mode_contains_only_connected_links_after_the_barrier() {
        let mut app = test_app();
        let first = app.world_mut().spawn(P2P::Inactive).id();
        let second = app.world_mut().spawn(P2P::Inactive).id();

        app.update();
        assert_eq!(mode(&app), &NetworkTopology::Undefined);

        // Declared but no session yet: there is no ready topology, which is what keeps gameplay
        // dormant until the barrier completes.
        app.world_mut().entity_mut(first).insert(P2P::Candidate);
        app.world_mut().entity_mut(second).insert(P2P::Candidate);
        app.update();

        assert_eq!(mode(&app), &NetworkTopology::Undefined);
        assert!(!mode(&app).is_p2p());

        app.world_mut().entity_mut(first).insert(P2P::Joined);
        app.update();
        assert_eq!(mode(&app), &NetworkTopology::Undefined);
        assert!(!mode(&app).is_p2p());

        // The barrier crosses its agreed tick.
        app.insert_resource(P2PSessionPhase::Active);
        app.world_mut().entity_mut(second).insert(P2P::Joined);
        app.world_mut()
            .entity_mut(first)
            .insert((RemoteId(PeerId::Local(1)), Connected));
        app.world_mut()
            .entity_mut(second)
            .insert((RemoteId(PeerId::Local(2)), Connected));
        app.update();
        assert_started_roster(&app, &[first, second]);
        assert!(mode(&app).is_p2p());

        app.world_mut().entity_mut(second).insert(Disconnected {
            reason: DisconnectedReason::UserRequested(Some("test".into())),
        });
        app.update();
        assert_started_roster(&app, &[first]);

        app.world_mut().despawn(second);
        app.update();
        assert_started_roster(&app, &[first]);

        app.world_mut().entity_mut(first).insert(Disconnected {
            reason: DisconnectedReason::UserRequested(Some("test".into())),
        });
        app.update();
        // The session itself is still started, so the local peer keeps playing with an empty
        // roster rather than losing its topology. Removing a peer from the *session* is separate
        // work; disconnecting a Link only removes it from the roster.
        assert_started_roster(&app, &[]);
        assert!(mode(&app).is_p2p());
        assert_eq!(app.world().entity(first).get::<P2P>(), Some(&P2P::Joined));
    }

    #[test]
    fn a_started_peer_keeps_its_started_roster_while_a_peer_is_joining() {
        let mut app = test_app();
        let joined = app
            .world_mut()
            .spawn((P2P::Joined, RemoteId(PeerId::Local(1)), Connected))
            .id();
        let joining = app
            .world_mut()
            .spawn((P2P::Candidate, RemoteId(PeerId::Local(2)), Connected))
            .id();
        app.insert_resource(P2PSessionPhase::Active);

        app.update();

        // The peer that is not playing yet is not part of the deterministic world, so the ready
        // roster is the started set. Reporting `Undefined` here would switch off input routing for
        // the started peer.
        assert_started_roster(&app, &[joined]);
        assert!(mode(&app).is_p2p());
        assert!(app.world().entity(joining).contains::<P2P>());

        // Leaving the session returns the application to the barrier rule.
        app.insert_resource(P2PSessionPhase::Stopped);
        app.update();
        assert_eq!(mode(&app), &NetworkTopology::Undefined);

        // Removing the candidate must not reactivate a stopped session through its Joined link.
        app.world_mut().despawn(joining);
        app.update();
        assert_eq!(mode(&app), &NetworkTopology::Undefined);
    }

    #[test]
    fn a_solo_session_reports_an_empty_p2p_roster() {
        let mut app = test_app();
        app.insert_resource(P2PSessionPhase::Active);

        app.update();

        // A solo peer must still have a ready P2P topology: `Undefined` would leave it with no
        // input route, so it could not capture or apply its own input.
        assert_started_roster(&app, &[]);
        assert!(mode(&app).is_p2p());
    }

    #[test]
    fn a_forming_cohort_exposes_its_roster_without_starting_gameplay() {
        let mut app = test_app();
        let candidate = app
            .world_mut()
            .spawn((P2P::Candidate, RemoteId(PeerId::Local(1)), Connected))
            .id();
        app.insert_resource(P2PSessionPhase::Starting);

        app.update();

        // The roster is exposed so that synchronization can follow the cohort, but this is not yet
        // a started peer: gameplay machinery must stay dormant.
        let roster = mode(&app)
            .p2p_roster()
            .expect("a forming cohort exposes its membership");
        assert_eq!(roster.phase, P2PSessionPhase::Starting);
        assert_eq!(roster.connected_candidates.as_slice(), &[candidate]);
        assert!(roster.started.is_empty());
        assert!(!mode(&app).is_p2p());
        assert!(mode(&app).started_p2p_roster().is_none());

        // The barrier becomes a started peer once it crosses its agreed tick.
        app.insert_resource(P2PSessionPhase::Active);
        app.world_mut().entity_mut(candidate).insert(P2P::Joined);
        app.update();
        assert_started_roster(&app, &[candidate]);
        assert!(mode(&app).is_p2p());
    }

    /// Assert that this application is a started peer whose roster is exactly `links`.
    fn assert_started_roster(app: &App, links: &[Entity]) {
        let roster = mode(app)
            .started_p2p_roster()
            .expect("a started session exposes its roster");
        assert_eq!(roster.started.as_slice(), links);
    }

    #[test]
    fn ready_p2p_and_conventional_roles_are_invalid() {
        let mut app = test_app();
        let peer = app
            .world_mut()
            .spawn((P2P::Candidate, RemoteId(PeerId::Local(1)), Connected))
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
    fn inactive_p2p_links_do_not_activate_or_conflict_with_conventional_topology() {
        let mut app = test_app();
        app.world_mut()
            .spawn((P2P::Inactive, RemoteId(PeerId::Local(1)), Connected));
        let client = connect_client(&mut app, 2);

        app.update();
        assert_eq!(mode(&app), &NetworkTopology::Client(client));
    }

    #[test]
    fn unready_conventional_roles_do_not_conflict_with_p2p() {
        let mut app = test_app();
        app.world_mut()
            .spawn((P2P::Candidate, RemoteId(PeerId::Local(1)), Connected));
        app.world_mut().spawn(Client);
        app.world_mut().spawn(Server::default());

        app.update();
        assert_eq!(mode(&app), &NetworkTopology::Undefined);
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
