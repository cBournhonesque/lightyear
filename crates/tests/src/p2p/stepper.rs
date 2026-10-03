//! Shared harness driving deterministic P2P sessions with lobby-based peer discovery.
//!
//! The transport is a parameter: [`TestTransport`] selects which lobby glue and endpoint each peer
//! gets. Test cases stay per transport (each asserts transport-specific behavior), but peer setup,
//! stepping, session gating, and port allocation are shared — including a single port allocator, so
//! parallel tests cannot collide on socket addresses.

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
use std::thread;

pub(crate) const TICK_DURATION: Duration = Duration::from_millis(10);
/// Steps allowed for endpoints to bind, peers to discover each other and a session to start.
///
/// Each step sleeps [`TICK_DURATION`], so this is a wall-clock budget of about 18 seconds. Real
/// sockets need real time, and the budget is generous because a starved machine should slow these
/// tests down rather than fail them.
pub(crate) const IO_ATTEMPTS: usize = 1800;

/// Which transport a [`Stepper`] peer dials and listens with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TestTransport {
    Udp,
    #[cfg(feature = "p2p_websocket")]
    WebSocket,
    #[cfg(feature = "p2p_webtransport")]
    WebTransport,
}

/// Hands each test its own port range so parallel tests cannot collide.
pub(crate) fn next_base_port() -> u16 {
    static NEXT: AtomicU16 = AtomicU16::new(0);
    // Stay away from the ephemeral range and from the examples' default (6000).
    20_000 + NEXT.fetch_add(100, Ordering::Relaxed)
}

/// The single address a peer is reachable at, which is also its identity.
pub(crate) fn peer_addr(base_port: u16, slot: u8) -> SocketAddr {
    SocketAddr::new(
        Ipv4Addr::LOCALHOST.into(),
        base_port
            .checked_add(u16::from(slot))
            .expect("base port plus slot must fit in u16"),
    )
}

pub(crate) fn peer_id(base_port: u16, slot: u8) -> PeerId {
    PeerId::Raw(peer_addr(base_port, slot))
}

pub(crate) fn lobby_id() -> LobbyId {
    LobbyId::from_bytes([7; 32])
}

pub(crate) struct LobbyPeer {
    pub(crate) app: App,
}

impl LobbyPeer {
    fn new(slot: u8, base_port: u16, policy: LobbyIdPolicy, transport: TestTransport) -> Self {
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

        // The one thing the application supplies: this peer's own address.
        let local = peer_addr(base_port, slot);
        match transport {
            TestTransport::Udp => {
                use lightyear_udp::endpoint::UdpEndpoint;
                use lightyear_udp::lobby::UdpLobbyPlugin;

                app.add_plugins(UdpLobbyPlugin);
                app.world_mut()
                    .spawn((UdpEndpoint::default(), LocalAddr(local)));
            }
            #[cfg(feature = "p2p_websocket")]
            TestTransport::WebSocket => {
                use lightyear::websocket::client::WebSocketScheme;
                use lightyear::websocket::endpoint::{ServerConfig, WebSocketEndpoint};
                use lightyear::websocket::lobby::WebSocketLobbyPlugin;
                use lightyear::websocket::prelude::client::ClientConfig;

                // Plain `ws`, no TLS: these tests are about the dial, not about certificates.
                app.add_plugins(WebSocketLobbyPlugin::new(
                    ClientConfig::builder().with_no_cert_validation(),
                    WebSocketScheme::Plain,
                ));
                app.world_mut().spawn((
                    WebSocketEndpoint {
                        config: ServerConfig::builder()
                            .with_bind_address(local)
                            .with_no_encryption(),
                    },
                    LocalAddr(local),
                ));
            }
            #[cfg(feature = "p2p_webtransport")]
            TestTransport::WebTransport => {
                use lightyear::webtransport::endpoint::WebTransportEndpoint;
                use lightyear::webtransport::lobby::WebTransportLobbyPlugin;

                // A self-signed identity per peer: the glue publishes its hash so the other side
                // can authenticate it, which is the whole reason this transport needs a dial
                // context.
                app.add_plugins(WebTransportLobbyPlugin);
                let certificate = Identity::self_signed(["localhost", "127.0.0.1", "::1"])
                    .expect("self-signed identity should be valid");
                app.world_mut()
                    .spawn((WebTransportEndpoint { certificate }, LocalAddr(local)));
            }
        }

        app.finish();
        app.cleanup();
        // Flush so the endpoint binds now: a peer that dials this one in an earlier frame cannot
        // lose the datagram to an unbound address.
        app.world_mut().flush();
        Self { app }
    }

    pub(crate) fn lobby(&self) -> &Lobby {
        self.app.world().resource::<Lobby>()
    }

    /// Whether this peer has started a deterministic P2P session.
    pub(crate) fn in_session(&self) -> bool {
        self.app
            .world()
            .resource::<NetworkingMetadata>()
            .mode
            .is_p2p()
    }

    /// How many Links this peer currently has up.
    pub(crate) fn connected_links(&mut self) -> usize {
        self.app
            .world_mut()
            .query_filtered::<Entity, (With<P2P>, With<Connected>)>()
            .iter(self.app.world())
            .count()
    }
}

pub(crate) struct Stepper {
    pub(crate) peers: Vec<LobbyPeer>,
    transport: TestTransport,
    now: Instant,
}

impl Stepper {
    pub(crate) fn new(
        transport: TestTransport,
        base_port: u16,
        policies: impl IntoIterator<Item = LobbyIdPolicy>,
    ) -> Self {
        Self {
            peers: policies
                .into_iter()
                .enumerate()
                .map(|(slot, policy)| LobbyPeer::new(slot as u8, base_port, policy, transport))
                .collect(),
            transport,
            now: Instant::now(),
        }
    }

    /// Tells `slot` to dial the given peers first. Every other peer must be reached by announcement.
    pub(crate) fn bootstrap(&mut self, base_port: u16, slot: u8, remotes: &[u8]) {
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

    /// Re-arms every peer this peer wanted but could not reach.
    pub(crate) fn retry_failed(&mut self, slot: u8) -> usize {
        self.peers[slot as usize]
            .app
            .world_mut()
            .resource_mut::<Lobby>()
            .retry_failed()
    }

    pub(crate) fn step(&mut self, n: usize) {
        for _ in 0..n {
            self.now += TICK_DURATION;
            for peer in &mut self.peers {
                peer.app
                    .insert_resource(TimeUpdateStrategy::ManualInstant(self.now));
                peer.app.update();
            }
            // The sockets are real, so the wall clock has to move with the simulated one.
            thread::sleep(TICK_DURATION);
        }
    }

    pub(crate) fn wait_until(&mut self, mut condition: impl FnMut(&mut Self) -> bool) {
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
    pub(crate) fn start_session_when_ready(&mut self, slots: &[u8]) {
        self.wait_until(|stepper| {
            slots.iter().all(|slot| {
                let lobby = stepper.peers[*slot as usize].lobby();
                let members = lobby.members().count();
                members + 1 == slots.len() && lobby.connected_members().count() == members
            })
        });
        for slot in slots {
            self.peers[*slot as usize].app.world_mut().trigger(P2PStart::default());
        }
    }

    pub(crate) fn wait_for_session(&mut self, slots: &[u8]) {
        self.wait_until(|stepper| {
            slots
                .iter()
                .all(|slot| stepper.peers[*slot as usize].in_session())
        });
    }
}

/// Drops both directions of recovery onto one shared script: kill the live session, require an
/// explicit retry, and prove the cohort is usable again.
///
/// `is_dialer` spots the Link that opened the original socket (the transport's client marker),
/// which decides who retries first. The WebTransport run additionally proves the retained dial
/// context still authenticates the original certificate.
pub(crate) fn explicit_retry_reconnects_members(
    stepper: &mut Stepper,
    base_port: u16,
    is_dialer: fn(&World, Entity) -> bool,
) {
    stepper.bootstrap(base_port, 0, &[1]);
    stepper.wait_until(|stepper| {
        stepper
            .peers
            .iter()
            .all(|peer| peer.lobby().connected_members().count() == 1)
    });

    let assert_links = |stepper: &mut Stepper| -> [Entity; 2] {
        core::array::from_fn(|slot| {
            let peer = &mut stepper.peers[slot];
            let remote = peer_id(base_port, 1 - slot as u8);
            assert!(!peer.in_session(), "recovery must not require admission");
            assert_eq!(peer.lobby().members().collect::<Vec<_>>(), vec![remote]);
            assert_eq!(
                peer.lobby().connected_members().collect::<Vec<_>>(),
                vec![remote]
            );
            assert_eq!(peer.connected_links(), 1);
            let world = peer.app.world_mut();
            assert_eq!(
                world
                    .query_filtered::<Entity, (With<P2P>, With<Linked>)>()
                    .iter(world)
                    .count(),
                1
            );
            let metadata = world.resource::<NetworkingMetadata>();
            assert_eq!(
                metadata.peer_map.keys().copied().collect::<Vec<_>>(),
                vec![remote],
                "only the advertised endpoint may own a peer-map entry"
            );
            let entity = metadata.peer_map[&remote];
            assert_eq!(world.get::<RemoteId>(entity), Some(&RemoteId(remote)));
            assert!(world.get::<P2P>(entity).is_some());
            assert!(world.get::<Linked>(entity).is_some());
            assert!(world.get::<Connected>(entity).is_some());
            entity
        })
    };
    let mut live_links = assert_links(stepper);
    let original_dialer = (0..2)
        .find(|slot| is_dialer(stepper.peers[*slot].app.world(), live_links[*slot]))
        .expect("one peer must have opened the original socket");
    let original_acceptor = 1 - original_dialer;
    assert!(
        stepper.peers[original_acceptor]
            .app
            .world()
            .get::<LinkOf>(live_links[original_acceptor])
            .is_some()
    );

    for (cycle, retrying) in [original_dialer, original_acceptor].into_iter().enumerate() {
        // Close only the actual session, not the endpoint: the other lobby must observe the
        // transport closure itself, and both listening sockets stay available for recovery.
        stepper.peers[original_dialer]
            .app
            .world_mut()
            .trigger(Unlink {
                entity: live_links[original_dialer],
                reason: UnlinkReason::UserRequested(None),
            });
        stepper.wait_until(|stepper| {
            stepper.peers.iter_mut().all(|peer| {
                peer.lobby().connected_members().count() == 0 && peer.connected_links() == 0
            })
        });
        stepper.step(10);
        for (slot, peer) in stepper.peers.iter_mut().enumerate() {
            assert_eq!(peer.lobby().connected_members().count(), 0);
            assert_eq!(
                peer.connected_links(),
                0,
                "recovery must wait for explicit retry"
            );
            assert_eq!(
                peer.lobby().members().collect::<Vec<_>>(),
                vec![peer_id(base_port, 1 - slot as u8)],
                "transport loss must preserve lobby membership"
            );
        }

        let remote = peer_id(base_port, 1 - retrying as u8);
        {
            let mut lobby = stepper.peers[retrying]
                .app
                .world_mut()
                .resource_mut::<Lobby>();
            if cycle == 0 {
                assert_eq!(lobby.retry_failed(), 1);
                assert_eq!(lobby.retry_failed(), 0, "already rearmed before dispatch");
            } else {
                // The original acceptor must be allowed to dial against the automatic tie-break,
                // even though its peer previously owned an outgoing Link for this identity.
                assert!(lobby.retry(remote));
                assert!(!lobby.retry(remote), "already rearmed before dispatch");
            }
            assert!(lobby.is_member(remote));
            assert!(!lobby.is_connected(remote));
        }
        stepper.wait_until(|stepper| {
            stepper
                .peers
                .iter()
                .all(|peer| peer.lobby().connected_members().count() == 1)
        });
        stepper.step(10);
        live_links = assert_links(stepper);
    }

    // Recovery must leave a usable cohort, not stale duplicate candidates for P2PStart.
    stepper.start_session_when_ready(&[0, 1]);
    stepper.wait_for_session(&[0, 1]);
}
