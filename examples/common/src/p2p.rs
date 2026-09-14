//! Shared setup for examples running as a direct P2P mesh.

use core::net::{Ipv4Addr, SocketAddr};
use core::ops::Range;
use core::time::Duration;

use bevy::prelude::*;
use lightyear::link::RecvLinkConditioner;
use lightyear::prelude::client::{ClientPlugins, RawClient};
use lightyear::prelude::*;

#[cfg(any(feature = "gui2d", feature = "gui3d"))]
use crate::client_renderer::ExampleClientRendererPlugin;

const MAX_P2P_PLAYERS: u8 = 4;
pub(crate) const DEFAULT_P2P_BASE_PORT: u16 = 6000;

/// Fixed roster used by an example running in direct P2P mode.
///
/// The initial example transport assigns compact numeric peer identities. Iroh can replace the
/// transport-specific identity construction later without changing the topology or game setup.
#[derive(Resource, Clone, Copy, Debug, PartialEq, Eq)]
pub struct P2PSettings {
    pub local_peer_id: u8,
    pub player_count: u8,
    /// Founding roster is application configuration, not persistent session policy.
    pub start_player_count: u8,
}

/// Present until every configured remote peer has responded and the P2P start barrier is entered.
#[derive(Resource)]
struct AwaitingP2PStart;

/// Present while this peer is joining an already-running session instead of forming one.
///
/// A joining peer never enters the start barrier: the session it is joining is already simulating,
/// and the barrier's roster check would either fail it or, worse, make it propose a new start.
#[derive(Resource)]
struct JoiningSession {
    bootstrap: PeerId,
}

impl P2PSettings {
    pub fn peer_ids(&self) -> Range<u8> {
        0..self.player_count
    }

    pub fn founding_peer_ids(&self) -> Range<u8> {
        0..self.start_player_count
    }

    pub fn local_id(&self) -> PeerId {
        PeerId::Entity(u64::from(self.local_peer_id))
    }
}

/// Build the stable input target shared by every peer for one roster member.
///
/// Remote targets are scoped to the Link that owns their input stream. The local target has no
/// receiver because this app captures and originates its inputs.
pub fn input_target_for_peer(
    settings: &P2PSettings,
    links: &Query<(Entity, &RemoteId), With<P2P>>,
    peer_id: u8,
    hash: u64,
) -> PreSpawned {
    let mut target = PreSpawned::new(hash);
    if peer_id == settings.local_peer_id {
        return target;
    }

    let remote_id = PeerId::Entity(u64::from(peer_id));
    let owner_link = links
        .iter()
        .find_map(|(entity, id)| (id.0 == remote_id).then_some(entity))
        .unwrap_or_else(|| panic!("missing P2P Link for roster peer {peer_id}"));
    target = target.for_receiver(owner_link);
    target
}

/// Add the client-side Lightyear plugins used by a direct P2P example.
pub(crate) fn configure_app(
    app: &mut App,
    tick_duration: Duration,
    _headless: bool,
    peer_id: u8,
    player_count: u8,
) {
    validate_roster(peer_id, player_count);
    app.add_plugins(ClientPlugins { tick_duration });
    app.insert_resource(P2PSettings {
        local_peer_id: peer_id,
        player_count,
        start_player_count: crate::automation::env_string("LIGHTYEAR_P2P_START_PEERS")
            .and_then(|value| value.parse().ok())
            .unwrap_or(player_count),
    });
    app.add_observer(|request: On<P2PJoinRequested>, mut commands: Commands| {
        commands.trigger(P2PJoinAdmission {
            peer_id: request.peer_id,
            request_id: request.request_id,
            epoch: request.epoch,
            result: Ok(()),
        });
    });

    #[cfg(any(feature = "gui2d", feature = "gui3d"))]
    if !_headless {
        app.add_plugins(ExampleClientRendererPlugin::new(format!(
            "P2P Peer {peer_id}"
        )));
    }
}

/// Schedule one directed raw UDP Link for every other member after protocol registration.
pub(crate) fn spawn_connections(
    app: &mut App,
    conditioner: &LinkConditionerConfig,
    peer_id: u8,
    player_count: u8,
    base_port: u16,
) {
    validate_roster(peer_id, player_count);
    let conditioner = conditioner.clone();
    app.add_systems(
        Startup,
        (
            move |mut commands: Commands| {
                let local_id = PeerId::Entity(u64::from(peer_id));
                for remote_peer_id in 0..player_count {
                    if remote_peer_id == peer_id {
                        continue;
                    }
                    let local_addr = peer_addr(base_port, peer_id, remote_peer_id);
                    let remote_addr = peer_addr(base_port, remote_peer_id, peer_id);
                    commands.spawn((
                        P2P::default(),
                        RawClient,
                        LocalId(local_id),
                        RemoteId(PeerId::Entity(u64::from(remote_peer_id))),
                        PingManager::default(),
                        LocalAddr(local_addr),
                        PeerAddr(remote_addr),
                        UdpIo::default(),
                        Link::default()
                            .with_conditioner(Some(RecvLinkConditioner::new(conditioner.clone()))),
                        Name::new(format!("P2P Link {peer_id} -> {remote_peer_id}")),
                    ));
                }
            },
            connect_links,
        )
            .chain(),
    );
    // `LIGHTYEAR_P2P_JOIN=<peer id>` makes this peer join a session that is already running
    // instead of forming one with the peers that are here.

    let bootstrap = crate::automation::env_string("LIGHTYEAR_P2P_JOIN")
        .and_then(|value| value.parse::<u64>().ok())
        .map(PeerId::Entity);
    match bootstrap {
        Some(bootstrap) => {
            app.insert_resource(JoiningSession { bootstrap });
            app.add_systems(Update, join_when_bootstrap_connected);
        }
        None => {
            app.insert_resource(AwaitingP2PStart);
            app.add_systems(Update, start_when_roster_connected);
        }
    }
}

/// Ask to join the running session once the peer that will admit us is reachable.
///
/// The request has to travel over a connected Link, so this waits for the transport rather than
/// firing at startup. Every other Link the roster needs is admitted by the join itself once the
/// bootstrap peer answers with the roster.
fn join_when_bootstrap_connected(
    mut commands: Commands,
    // Taken by this system once it fires, so it must not be a required parameter.
    joining: Option<Res<JoiningSession>>,
    links: Query<(&RemoteId, Has<Connected>), With<P2P>>,
) {
    let Some(joining) = joining else {
        return;
    };
    let connected = links
        .iter()
        .any(|(remote_id, connected)| remote_id.0 == joining.bootstrap && connected);
    if !connected {
        return;
    }
    tracing::info!(
        bootstrap = ?joining.bootstrap,
        "P2P bootstrap peer reachable; asking to join the running session"
    );
    commands.remove_resource::<JoiningSession>();
    commands.trigger(lightyear::p2p::P2PJoin {
        bootstrap: joining.bootstrap,
    });
}

fn validate_roster(peer_id: u8, player_count: u8) {
    assert!(
        (2..=MAX_P2P_PLAYERS).contains(&player_count),
        "P2P player_count must be between 2 and {MAX_P2P_PLAYERS}"
    );
    assert!(
        peer_id < player_count,
        "P2P peer_id {peer_id} is outside the {player_count}-player roster"
    );
}

fn peer_addr(base_port: u16, local_peer_id: u8, remote_peer_id: u8) -> SocketAddr {
    let offset = u16::from(local_peer_id) * u16::from(MAX_P2P_PLAYERS) + u16::from(remote_peer_id);
    let port = base_port
        .checked_add(offset)
        .expect("P2P base port plus roster offset must fit in u16");
    SocketAddr::new(Ipv4Addr::LOCALHOST.into(), port)
}

fn connect_links(mut commands: Commands, links: Query<Entity, With<P2P>>) {
    for entity in &links {
        commands.trigger(Connect { entity });
    }
}

/// Start when every explicitly selected founding peer has replied to a ping.
fn start_when_roster_connected(
    mut commands: Commands,
    settings: Res<P2PSettings>,
    awaiting_start: Option<Res<AwaitingP2PStart>>,
    links: Query<(&RemoteId, Has<Connected>, &PingManager), With<P2P>>,
) {
    if awaiting_start.is_none() {
        return;
    }
    // Wait for every peer in the cohort: the barrier needs all of them, so starting before they are
    // reachable would only make it wait. A peer outside the cohort is not waited for at all — that
    // is the whole point of declaring one.
    let required = settings
        .founding_peer_ids()
        .filter(|peer| *peer != settings.local_peer_id)
        .count();
    let ready = links
        .iter()
        .filter(|(remote, connected, ping)| {
            settings
                .founding_peer_ids()
                .any(|peer| remote.0 == PeerId::Entity(u64::from(peer)))
                && *connected
                && ping.latency_samples_recv() > 0
        })
        .count();
    if ready < required {
        return;
    }

    tracing::info!(
        ready,
        required,
        player_count = settings.player_count,
        "P2P roster ready; starting session negotiation"
    );
    commands.remove_resource::<AwaitingP2PStart>();
    commands.trigger(P2PStart {
        cohort: NetworkTarget::Only(
            settings
                .founding_peer_ids()
                .map(|peer| PeerId::Entity(u64::from(peer)))
                .collect(),
        ),
    });
}
