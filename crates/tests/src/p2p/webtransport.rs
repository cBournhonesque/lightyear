//! Lobby discovery over the real WebTransport transport.
//!
//! The UDP tests prove the lobby is transport-agnostic; this one proves the WebTransport glue dials.
//! What differs is worth stating, because it is what the glue exists for:
//!
//! - WebTransport authenticates the endpoint with a certificate, so a peer must additionally
//!   publish the hash of its own. That is what the announce carries, and what the glue turns into
//!   the client's expected digest.
//! - A stream transport cannot tell who dialed it: the accepted session reports the ephemeral port
//!   the peer connected from, not the endpoint it listens on. The announce names its sender, and the
//!   lobby re-keys the Link to it — without that the two peers would disagree on the roster.
//! - Only `RawClient` and Steam promote `Linked` to `Connected`, so the glue does it.

use crate::protocol::ProtocolPlugin;

use core::net::{Ipv4Addr, SocketAddr};
use core::sync::atomic::{AtomicU16, Ordering};
use core::time::Duration;

use bevy::MinimalPlugins;
use bevy::log::LogPlugin;
use bevy::platform::time::Instant;
use bevy::prelude::*;
use bevy::state::app::StatesPlugin;
use bevy::time::TimeUpdateStrategy;
use lightyear::p2p::{Lobby, LobbyId, LobbyIdPolicy, LobbyPlugin};
use lightyear::prelude::*;
use lightyear::webtransport::endpoint::WebTransportEndpoint;
use lightyear::webtransport::lobby::WebTransportLobbyPlugin;
use std::thread;
use test_log::test;

const TICK_DURATION: Duration = Duration::from_millis(10);
/// Wall-clock budget of about 18 seconds; real sockets need real time.
const IO_ATTEMPTS: usize = 1800;

fn next_base_port() -> u16 {
    static NEXT: AtomicU16 = AtomicU16::new(0);
    20_000 + NEXT.fetch_add(100, Ordering::Relaxed)
}

fn peer_addr(base_port: u16, slot: u8) -> SocketAddr {
    SocketAddr::new(
        Ipv4Addr::LOCALHOST.into(),
        base_port
            .checked_add(u16::from(slot))
            .expect("base port plus slot must fit in u16"),
    )
}

fn peer_id(base_port: u16, slot: u8) -> PeerId {
    PeerId::Raw(peer_addr(base_port, slot))
}

fn lobby_id() -> LobbyId {
    LobbyId::from_bytes([7; 32])
}

struct LobbyPeer {
    app: App,
}

impl LobbyPeer {
    fn new(slot: u8, base_port: u16, policy: LobbyIdPolicy) -> Self {
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            TransformPlugin,
            StatesPlugin,
            LogPlugin::default(),
        ));
        app.add_plugins(client::ClientPlugins {
            tick_duration: TICK_DURATION,
        });
        app.add_plugins(ProtocolPlugin {
            avian_mode: Default::default(),
        });
        app.add_plugins(LobbyPlugin::new(policy));
        app.add_plugins(WebTransportLobbyPlugin);

        // The one thing the application supplies: this peer's own address.
        // A self-signed identity per peer: the glue publishes its hash so the other side can
        // authenticate it, which is the whole reason this transport needs a dial context.
        let local = peer_addr(base_port, slot);
        let certificate = Identity::self_signed(["localhost", "127.0.0.1", "::1"])
            .expect("self-signed identity should be valid");
        app.world_mut()
            .spawn((WebTransportEndpoint { certificate }, LocalAddr(local)));

        app.finish();
        app.cleanup();
        app.world_mut().flush();
        Self { app }
    }

    fn lobby(&self) -> &Lobby {
        self.app.world().resource::<Lobby>()
    }

    fn in_session(&self) -> bool {
        self.app
            .world()
            .resource::<NetworkingMetadata>()
            .mode
            .is_p2p()
    }

    fn connected_links(&mut self) -> usize {
        self.app
            .world_mut()
            .query_filtered::<Entity, (With<P2P>, With<Connected>)>()
            .iter(self.app.world())
            .count()
    }
}

struct Stepper {
    peers: Vec<LobbyPeer>,
    now: Instant,
}

impl Stepper {
    fn new(base_port: u16, policies: impl IntoIterator<Item = LobbyIdPolicy>) -> Self {
        Self {
            peers: policies
                .into_iter()
                .enumerate()
                .map(|(slot, policy)| LobbyPeer::new(slot as u8, base_port, policy))
                .collect(),
            now: Instant::now(),
        }
    }

    fn bootstrap(&mut self, base_port: u16, slot: u8, remotes: &[u8]) {
        let peers: Vec<PeerId> = remotes
            .iter()
            .map(|remote| peer_id(base_port, *remote))
            .collect();
        self.peers[slot as usize]
            .app
            .world_mut()
            .resource_mut::<Lobby>()
            .add_bootstrap(peers);
    }

    fn step(&mut self, n: usize) {
        for _ in 0..n {
            self.now += TICK_DURATION;
            for peer in &mut self.peers {
                peer.app
                    .insert_resource(TimeUpdateStrategy::ManualInstant(self.now));
                peer.app.update();
            }
            thread::sleep(TICK_DURATION);
        }
    }

    fn wait_until(&mut self, mut condition: impl FnMut(&mut Self) -> bool) {
        for _ in 0..IO_ATTEMPTS {
            if condition(self) {
                return;
            }
            self.step(1);
        }
        panic!("the lobby condition was not met within the attempt budget");
    }

    fn start_session_when_ready(&mut self, slots: &[u8]) {
        self.wait_until(|stepper| {
            slots.iter().all(|slot| {
                let lobby = stepper.peers[*slot as usize].lobby();
                let members = lobby.members().count();
                members + 1 == slots.len() && lobby.connected_members().count() == members
            })
        });
        for slot in slots {
            self.peers[*slot as usize].app.world_mut().trigger(P2PStart);
        }
    }

    fn wait_for_session(&mut self, slots: &[u8]) {
        self.wait_until(|stepper| {
            slots
                .iter()
                .all(|slot| stepper.peers[*slot as usize].in_session())
        });
    }
}

#[test]
fn two_peers_discover_each_other_over_webtransport_and_start_a_session() {
    let base_port = next_base_port();
    let mut stepper = Stepper::new(
        base_port,
        [
            LobbyIdPolicy::Pinned(Some(lobby_id())),
            LobbyIdPolicy::Adopt,
        ],
    );
    // Only the host is seeded. The dial tie-break picks the one direction.
    stepper.bootstrap(base_port, 0, &[1]);

    stepper.start_session_when_ready(&[0, 1]);
    stepper.wait_for_session(&[0, 1]);

    {
        let first = stepper.peers[0].lobby();
        let second = stepper.peers[1].lobby();
        assert_eq!(first.id(), Some(lobby_id()));
        assert_eq!(second.id(), Some(lobby_id()));
        // Both sides call the *endpoint* address of the peer their peer id, even though the acceptor
        // only ever saw the dialer's ephemeral port. That agreement is what the re-key buys.
        assert_eq!(
            first.roster(),
            vec![peer_id(base_port, 0), peer_id(base_port, 1)]
        );
        assert_eq!(first.roster(), second.roster());
    }
    assert_eq!(stepper.peers[0].connected_links(), 1);
    assert_eq!(stepper.peers[1].connected_links(), 1);
}

#[test]
fn the_webtransport_peers_agree_on_slots() {
    let base_port = next_base_port();
    let mut stepper = Stepper::new(
        base_port,
        [
            LobbyIdPolicy::Pinned(Some(lobby_id())),
            LobbyIdPolicy::Adopt,
        ],
    );
    stepper.bootstrap(base_port, 0, &[1]);
    stepper.start_session_when_ready(&[0, 1]);
    stepper.wait_for_session(&[0, 1]);

    let slots_of = |slot: u8| -> Vec<(PeerId, Option<u8>)> {
        let lobby = stepper.peers[slot as usize].lobby();
        lobby
            .roster()
            .iter()
            .map(|peer| (*peer, lobby.slot_of(*peer)))
            .collect()
    };
    assert_eq!(slots_of(0), slots_of(1));
    assert_ne!(
        stepper.peers[0].lobby().local_slot(),
        stepper.peers[1].lobby().local_slot(),
    );
}

#[test]
fn three_peers_join_one_seed_without_identity_collisions() {
    let base_port = next_base_port();
    let mut stepper = Stepper::new(
        base_port,
        [
            LobbyIdPolicy::Pinned(Some(lobby_id())),
            LobbyIdPolicy::Adopt,
            LobbyIdPolicy::Adopt,
        ],
    );
    stepper.bootstrap(base_port, 1, &[0]);
    stepper.bootstrap(base_port, 2, &[0]);
    stepper.start_session_when_ready(&[0, 1, 2]);
    stepper.wait_for_session(&[0, 1, 2]);
    for (slot, peer) in stepper.peers.iter_mut().enumerate() {
        assert_eq!(peer.connected_links(), 2);
        let local = peer_id(base_port, slot as u8);
        let expected: Vec<_> = (0..3)
            .map(|slot| peer_id(base_port, slot))
            .filter(|id| *id != local)
            .collect();
        let metadata = peer.app.world().resource::<NetworkingMetadata>();
        let mut actual: Vec<_> = metadata.peer_map.keys().copied().collect();
        actual.sort_unstable();
        assert_eq!(
            actual, expected,
            "all peers must resolve by endpoint identity"
        );
    }
}
