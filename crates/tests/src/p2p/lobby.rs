//! Lobby discovery over the real UDP transport.
//!
//! These tests are the proof that the lobby is transport-agnostic: the same [`LobbyPlugin`] that
//! other transports drive is driven here by UDP's glue, through the same [`DialPeer`] event and the
//! same scope — Links carrying [`P2P`]. Nothing in `lightyear_p2p` knows about UDP.
//!
//! What is specific to UDP is that a peer's address *is* its identity: every peer binds one endpoint
//! socket, announces carry `PeerId::Raw`, and dialing a discovered peer needs no configuration. The
//! tests that matter most here are the ones where a peer is reached without the application ever
//! being told where it is.

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
use lightyear_udp::endpoint::UdpEndpoint;
use lightyear_udp::lobby::UdpLobbyPlugin;
use std::thread;
use test_log::test;

const TICK_DURATION: Duration = Duration::from_millis(10);
/// Steps allowed for endpoints to bind, peers to discover each other and a session to start.
///
/// Each step sleeps [`TICK_DURATION`], so this is a wall-clock budget of about 18 seconds. Real UDP
/// sockets need real time, and the budget is generous because a starved machine should slow these
/// tests down rather than fail them.
const IO_ATTEMPTS: usize = 1800;

/// Hands each test its own port range so parallel tests cannot collide.
fn next_base_port() -> u16 {
    static NEXT: AtomicU16 = AtomicU16::new(0);
    // Stay away from the ephemeral range and from the examples' default (6000).
    20_000 + NEXT.fetch_add(100, Ordering::Relaxed)
}

/// The single address a peer is reachable at, which is also its identity.
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

fn other_lobby_id() -> LobbyId {
    LobbyId::from_bytes([9; 32])
}

struct LobbyPeer {
    app: App,
    slot: u8,
    base_port: u16,
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
        // The shared test protocol registers inputs, which is what creates the resources the
        // prediction plugin expects. Without it the app panics on a missing `LastConfirmedInput`,
        // which has nothing to do with the lobby.
        app.add_plugins(ProtocolPlugin {
            avian_mode: Default::default(),
        });
        app.add_plugins(LobbyPlugin::new(policy));
        app.add_plugins(UdpLobbyPlugin);

        // The whole application-supplied configuration for reaching other peers: this peer's own
        // socket. Binding port 0 would also do; the endpoint reports the address it got.
        app.world_mut().spawn((
            UdpEndpoint::default(),
            LocalAddr(peer_addr(base_port, slot)),
        ));

        app.finish();
        app.cleanup();
        // Bind the socket now so a peer that dials this one in an earlier frame cannot lose the
        // datagram to an unbound address.
        app.world_mut().flush();

        Self {
            app,
            slot,
            base_port,
        }
    }

    fn lobby(&self) -> &Lobby {
        self.app.world().resource::<Lobby>()
    }

    /// Whether this peer has started a deterministic P2P session.
    fn in_session(&self) -> bool {
        self.app
            .world()
            .resource::<NetworkingMetadata>()
            .mode
            .is_p2p()
    }

    /// How many Links this peer currently has up.
    fn connected_links(&mut self) -> usize {
        self.app
            .world_mut()
            .query_filtered::<Entity, (With<P2P>, With<Connected>)>()
            .iter(self.app.world())
            .count()
    }

    /// Re-arms every peer this peer wanted but could not reach.
    fn retry_failed(&mut self) -> usize {
        self.app.world_mut().resource_mut::<Lobby>().retry_failed()
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

    /// Starts a peer while the others are already running, as a late arrival would.
    fn add_peer(&mut self, slot: u8, base_port: u16, policy: LobbyIdPolicy) {
        self.peers.push(LobbyPeer::new(slot, base_port, policy));
    }

    /// Tells `slot` to dial the given peers first. Every other peer must be reached by announcement.
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

    fn retry_failed(&mut self, slot: u8) -> usize {
        self.peers[slot as usize].retry_failed()
    }

    fn step(&mut self, n: usize) {
        for _ in 0..n {
            self.now += TICK_DURATION;
            for peer in &mut self.peers {
                peer.app
                    .insert_resource(TimeUpdateStrategy::ManualInstant(self.now));
                peer.app.update();
            }
            // The UDP sockets are real, so the wall clock has to move with the simulated one.
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

    /// Acts as the application: once every peer sees the same complete roster, start the session.
    ///
    /// This is deliberately *not* the lobby's job — the lobby only discovers peers and reports
    /// membership. The test writes the application's own readiness rule, which is the point of the
    /// split.
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
fn two_peers_discover_each_other_over_udp_and_start_a_session() {
    let base_port = next_base_port();
    let mut stepper = Stepper::new(
        base_port,
        [
            LobbyIdPolicy::Pinned(Some(lobby_id())),
            LobbyIdPolicy::Adopt,
        ],
    );
    // Only one side is seeded. The peer that is dialed learns the dialer from the datagram it
    // receives, so a UDP pair does not need a dial in each direction.
    stepper.bootstrap(base_port, 0, &[1]);

    // Nothing is connected yet: the dialed peer is wanted, but the roster is empty because the local
    // id is only learned from a connected Link.
    assert_eq!(
        stepper.peers[0].lobby().waiting().collect::<Vec<_>>(),
        vec![peer_id(base_port, 1)]
    );
    assert!(stepper.peers[0].lobby().roster().is_empty());

    stepper.start_session_when_ready(&[0, 1]);
    stepper.wait_for_session(&[0, 1]);

    {
        let first = stepper.peers[0].lobby();
        let second = stepper.peers[1].lobby();

        // The joiner adopted the lobby it was invited to rather than inventing one.
        assert_eq!(first.id(), Some(lobby_id()));
        assert_eq!(second.id(), Some(lobby_id()));

        // Both derived the same two-member roster, so `P2PStart` froze the same cohort and the
        // session's roster hash agreed. Each peer's identity is its endpoint address, so the two
        // sides of the roster are the same two `PeerId::Raw` values.
        assert_eq!(first.roster(), second.roster());
        assert_eq!(
            first.roster(),
            vec![peer_id(base_port, 0), peer_id(base_port, 1)]
        );

        assert_eq!(first.connected_members().count(), 1);
        assert!(first.waiting().next().is_none(), "nobody is left waiting");
    }
    assert_eq!(stepper.peers[0].connected_links(), 1);
    assert_eq!(stepper.peers[1].connected_links(), 1);
}

#[test]
fn the_two_peers_agree_on_slots() {
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

    // Slots come from the sorted roster, so both peers assign the same slot to the same peer
    // without exchanging anything but membership.
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
        "the two peers must occupy different slots"
    );
}

#[test]
fn a_third_peer_is_discovered_and_dialed_without_being_configured() {
    // Only peer 0 is seeded. Peers 1 and 2 are never told about each other — not their addresses,
    // not that they exist. The Link between them can only come from peer 0 announcing who it knows,
    // and the address to dial comes from the announced `PeerId` itself.
    let base_port = next_base_port();
    let mut stepper = Stepper::new(
        base_port,
        [
            LobbyIdPolicy::Pinned(Some(lobby_id())),
            LobbyIdPolicy::Adopt,
            LobbyIdPolicy::Adopt,
        ],
    );
    stepper.bootstrap(base_port, 0, &[1, 2]);

    stepper.start_session_when_ready(&[0, 1, 2]);
    stepper.wait_for_session(&[0, 1, 2]);

    let slots = [0u8, 1, 2];
    for slot in slots {
        let lobby = stepper.peers[slot as usize].lobby();
        // Every peer sees the two others as members, and never itself: the member set is the roster
        // minus the local peer.
        for other in slots.into_iter().filter(|other| *other != slot) {
            assert!(
                lobby.is_member(peer_id(base_port, other)),
                "peer {slot} should know peer {other} as a member"
            );
        }
        assert!(
            !lobby.is_member(peer_id(base_port, slot)),
            "peer {slot} should not list itself as a member"
        );
        assert_eq!(
            lobby.connected_members().count(),
            2,
            "peer {slot} should be connected to both other members"
        );
    }
    for slot in slots {
        assert_eq!(
            stepper.peers[slot as usize].connected_links(),
            2,
            "peer {slot} should have a Link to both other peers"
        );
    }

    // All three derived the same roster, so the barrier could agree.
    let rosters: Vec<Vec<PeerId>> = stepper
        .peers
        .iter()
        .map(|peer| peer.lobby().roster())
        .collect();
    assert_eq!(rosters[0], rosters[1]);
    assert_eq!(rosters[1], rosters[2]);
    assert_eq!(rosters[0].len(), 3);
}

#[test]
fn a_peer_pinned_to_another_lobby_is_excluded_from_discovery() {
    // A stranger dials the founder but insists on a different lobby. This is what the lobby id is
    // for: without it the two gatherings would merge into one peer set.
    let base_port = next_base_port();
    let mut stepper = Stepper::new(
        base_port,
        [
            LobbyIdPolicy::Pinned(Some(lobby_id())),
            LobbyIdPolicy::Adopt,
            LobbyIdPolicy::Pinned(Some(other_lobby_id())),
        ],
    );
    stepper.bootstrap(base_port, 0, &[1, 2]);

    // Let the announces be exchanged and reacted to.
    stepper.step(400);

    // The founder and its rightful joiner form one lobby; the stranger is recorded as foreign,
    // contributes nothing to the roster, and is never advertised onward.
    assert_eq!(
        stepper.peers[0].lobby().members().collect::<Vec<_>>(),
        vec![peer_id(base_port, 1)]
    );
    assert_eq!(stepper.peers[0].lobby().id(), Some(lobby_id()));
    assert_eq!(stepper.peers[1].lobby().id(), Some(lobby_id()));
    assert!(
        !stepper.peers[0]
            .lobby()
            .roster()
            .contains(&peer_id(base_port, 2))
    );
    assert!(
        !stepper.peers[1]
            .lobby()
            .roster()
            .contains(&peer_id(base_port, 2))
    );
    assert!(stepper.peers[2].lobby().members().next().is_none());

    // The lobby leaves the stranger's Link alone: what to do with a connected peer from another
    // lobby is the application's decision.
    assert!(
        stepper.peers[0].lobby().is_connected(peer_id(base_port, 2)),
        "the lobby does not unlink a peer from another lobby"
    );
}

#[test]
fn an_unreachable_peer_stays_unconfirmed_and_is_retried() {
    // Slot 9 is never started, so nothing is listening at its address: the dial goes out and no
    // announce ever comes back.
    let base_port = next_base_port();
    let mut stepper = Stepper::new(base_port, [LobbyIdPolicy::Pinned(Some(lobby_id()))]);
    stepper.bootstrap(base_port, 0, &[9]);

    stepper.step(200);
    assert_eq!(
        stepper.peers[0].lobby().members().count(),
        0,
        "nothing replied, so the peer was never confirmed"
    );
    // The peer has a Link — UDP has no handshake, so dialing creates a usable one — but nothing has
    // confirmed anyone is there, which is the difference between a Link and a member.
    assert!(stepper.peers[0].lobby().is_connected(peer_id(base_port, 9)));
    assert!(!stepper.peers[0].lobby().is_member(peer_id(base_port, 9)));

    assert_eq!(
        stepper.retry_failed(0),
        1,
        "the peer that was never confirmed is the one re-armed"
    );
    assert_eq!(
        stepper.retry_failed(0),
        0,
        "an already re-armed peer is not counted twice"
    );

    // Retrying does not conjure the peer: it stays unconfirmed while nothing is there.
    stepper.step(50);
    assert_eq!(stepper.peers[0].lobby().members().count(), 0);
}
