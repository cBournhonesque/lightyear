//! Shared setup for examples running as a P2P mesh with lobby-based discovery.
//!
//! Every peer opens an endpoint that other peers can connect to, no matter which
//! transport is selected (UDP, WebSocket or WebTransport). Peers find each other
//! through [`Lobby`](lightyear::p2p::Lobby) announces instead of a precomputed
//! roster: run one peer with no `--peer` to open a lobby, then run the others
//! with `--peer <addr>` pointing at any peer that is already in the lobby.
//! Every example lobby uses the same [`P2P_LOBBY_ID`], so no lobby id ever has
//! to be exchanged out of band.

use core::fmt::Write as _;
use core::net::{Ipv4Addr, SocketAddr};
use core::time::Duration;

use bevy::prelude::*;
use clap::ValueEnum;
use lightyear::core::id::peer_id_from_socket;
use lightyear::p2p::Lobby;
use lightyear::prelude::client::ClientPlugins;
use lightyear::prelude::*;
#[cfg(not(target_family = "wasm"))]
use lightyear::websocket::endpoint::{ServerConfig, WebSocketEndpoint};
#[cfg(not(target_family = "wasm"))]
use lightyear::webtransport::endpoint::WebTransportEndpoint;
#[cfg(not(target_family = "wasm"))]
use lightyear::webtransport::prelude::Identity;
#[cfg(all(feature = "udp", not(target_family = "wasm")))]
use lightyear_udp::prelude::endpoint::UdpEndpoint;

#[cfg(any(feature = "gui2d", feature = "gui3d"))]
use crate::client_renderer::ExampleClientRendererPlugin;

const MAX_P2P_PLAYERS: u8 = 4;
pub(crate) const DEFAULT_P2P_PORT: u16 = 6000;

/// Lobby identity shared by every example peer.
///
/// A membership token, not an address: nothing ever dials it. All example peers
/// pin this id, so a peer joins a lobby by dialing any member's endpoint
/// address (via `--peer`), never by naming the lobby itself.
pub const P2P_LOBBY_ID: LobbyId = LobbyId::from_bytes([0; 32]);

/// Which transport a P2P example peer listens on and dials others with.
///
/// Every variant exposes a dialable endpoint. Its native address is advertised
/// separately from the opaque routing alias.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum P2PTransport {
    /// Raw UDP datagrams. Native targets only.
    #[cfg(all(feature = "udp", not(target_family = "wasm")))]
    #[value(name = "udp")]
    Udp,
    /// Plain (unencrypted) WebSocket.
    #[value(name = "websocket")]
    WebSocket,
    /// WebTransport over QUIC with a per-peer self-signed certificate.
    #[value(name = "webtransport")]
    WebTransport,
}

impl Default for P2PTransport {
    fn default() -> Self {
        #[cfg(all(feature = "udp", not(target_family = "wasm")))]
        return P2PTransport::Udp;
        #[cfg(not(all(feature = "udp", not(target_family = "wasm"))))]
        return P2PTransport::WebSocket;
    }
}

impl core::fmt::Display for P2PTransport {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            #[cfg(all(feature = "udp", not(target_family = "wasm")))]
            P2PTransport::Udp => write!(f, "udp"),
            P2PTransport::WebSocket => write!(f, "websocket"),
            P2PTransport::WebTransport => write!(f, "webtransport"),
        }
    }
}

/// How an example peer reaches its lobby.
///
/// The roster is discovered, never configured: `seed` names at most one peer
/// that is already in the lobby. `None` opens a new lobby that later peers can
/// join. `expected_players` only gates the session start barrier.
#[derive(Resource, Clone, Copy, Debug)]
pub struct P2PSettings {
    pub expected_players: u8,
    pub transport: P2PTransport,
    pub local_port: u16,
    pub seed: Option<SocketAddr>,
    pub seed_certificate_digest: Option<[u8; 32]>,
}

/// Parse a certificate pin supplied out of band by a WebTransport seed.
pub(crate) fn parse_certificate_digest(value: &str) -> Result<[u8; 32], String> {
    if value.len() != 64 || !value.is_ascii() {
        return Err("certificate digest must contain exactly 64 hexadecimal digits".into());
    }
    let mut digest = [0; 32];
    for (index, byte) in digest.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16)
            .map_err(|_| "certificate digest must contain only hexadecimal digits")?;
    }
    Ok(digest)
}

/// Present until the discovered roster is complete and the session starts.
#[derive(Resource)]
struct AwaitingP2PStart;

/// Build the stable input target shared by every peer for one roster member.
///
/// `peer` is the lobby identity of the roster member and `hash` its stable
/// input-wire identity (hash base plus roster slot). Remote targets are scoped
/// to the Link that owns their input stream. The local target has no receiver
/// because this app captures and originates its inputs.
pub fn input_target_for_peer(
    lobby: &Lobby,
    links: &Query<(Entity, &RemoteId), With<P2P>>,
    peer: PeerId,
    hash: u64,
) -> PreSpawned {
    let mut target = PreSpawned::new(hash);
    if lobby.local() == Some(peer) {
        return target;
    }

    let owner_link = links
        .iter()
        .find_map(|(entity, id)| (id.0 == peer).then_some(entity))
        .unwrap_or_else(|| panic!("missing P2P Link for lobby peer {peer:?}"));
    target = target.for_receiver(owner_link);
    target
}

/// Add the client-side Lightyear plugins and lobby discovery used by a P2P example.
pub(crate) fn configure_app(
    app: &mut App,
    tick_duration: Duration,
    _headless: bool,
    settings: P2PSettings,
) {
    validate_settings(settings.expected_players);
    app.add_plugins(ClientPlugins { tick_duration });
    app.add_plugins(LobbyPlugin::new(LobbyIdPolicy::Pinned(Some(P2P_LOBBY_ID))));
    match settings.transport {
        #[cfg(all(feature = "udp", not(target_family = "wasm")))]
        P2PTransport::Udp => {
            app.add_plugins(lightyear_udp::lobby::UdpLobbyPlugin);
        }
        #[cfg(not(target_family = "wasm"))]
        P2PTransport::WebSocket => {
            app.add_plugins(lightyear::websocket::lobby::WebSocketLobbyPlugin::new(
                lightyear::websocket::prelude::client::ClientConfig::builder()
                    .with_no_cert_validation(),
                lightyear::websocket::prelude::client::WebSocketScheme::Plain,
            ));
        }
        #[cfg(not(target_family = "wasm"))]
        P2PTransport::WebTransport => {
            app.add_plugins(lightyear::webtransport::lobby::WebTransportLobbyPlugin);
        }
        #[cfg(target_family = "wasm")]
        _ => panic!("P2P endpoints are not supported on wasm"),
    }
    app.insert_resource(settings);

    #[cfg(any(feature = "gui2d", feature = "gui3d"))]
    if !_headless {
        app.add_plugins(ExampleClientRendererPlugin::new(format!(
            "P2P Peer ({} players expected)",
            settings.expected_players
        )));
    }
}

/// Open this peer's endpoint and join (or open) the lobby.
///
/// The endpoint is what other peers connect to, whichever transport is
/// selected. When `settings.seed` names a lobby member, the lobby dials it and
/// learns everyone else through announces; otherwise this peer waits to be
/// dialed. Nothing else is configured per peer.
pub(crate) fn spawn_connections(
    app: &mut App,
    conditioner: &LinkConditionerConfig,
    settings: P2PSettings,
) {
    validate_settings(settings.expected_players);
    let conditioner = RecvLinkConditioner::new(conditioner.clone());
    // Advertise a dialable native address; the routing alias never contains it.
    let bind = SocketAddr::new(Ipv4Addr::LOCALHOST.into(), settings.local_port);
    match settings.transport {
        #[cfg(all(feature = "udp", not(target_family = "wasm")))]
        P2PTransport::Udp => {
            app.world_mut().spawn((
                Endpoint::new(Some(conditioner)),
                UdpEndpoint::new(bind),
                Name::new("P2P Endpoint"),
            ));
        }
        #[cfg(not(target_family = "wasm"))]
        P2PTransport::WebSocket => {
            let config = ServerConfig::builder()
                .with_bind_address(bind)
                .with_no_encryption();
            app.world_mut().spawn((
                Endpoint::new(Some(conditioner)),
                WebSocketEndpoint { config },
                Name::new("P2P Endpoint"),
            ));
        }
        #[cfg(not(target_family = "wasm"))]
        P2PTransport::WebTransport => {
            let certificate = Identity::self_signed(["localhost", "127.0.0.1", "::1"]).unwrap();
            let hash = certificate.certificate_chain().as_slice()[0].hash();
            let mut digest = String::with_capacity(64);
            for byte in hash.as_ref() {
                write!(&mut digest, "{byte:02x}").expect("writing to a String cannot fail");
            }
            info!("P2P WebTransport certificate digest: {digest}");
            app.world_mut().spawn((
                Endpoint::new(Some(conditioner)),
                WebTransportEndpoint {
                    certificate,
                    bind_addr: bind,
                },
                Name::new("P2P Endpoint"),
            ));
        }
        #[cfg(target_family = "wasm")]
        _ => panic!("P2P endpoints are not supported on wasm"),
    }
    if let Some(seed) = settings.seed {
        let (domain, context): (&[u8], _) = match settings.transport {
            #[cfg(all(feature = "udp", not(target_family = "wasm")))]
            P2PTransport::Udp => (b"udp", lightyear_udp::lobby::dial_context(seed)),
            #[cfg(not(target_family = "wasm"))]
            P2PTransport::WebSocket => (
                b"websocket",
                lightyear::websocket::lobby::dial_context(
                    seed,
                    lightyear::websocket::prelude::client::WebSocketScheme::Plain,
                ),
            ),
            #[cfg(not(target_family = "wasm"))]
            P2PTransport::WebTransport => (
                b"webtransport",
                lightyear::webtransport::lobby::dial_context(
                    seed,
                    &settings.seed_certificate_digest.expect(
                        "WebTransport --peer requires --peer-certificate-digest from the seed",
                    ),
                ),
            ),
            #[cfg(target_family = "wasm")]
            _ => panic!("P2P endpoints are not supported on wasm"),
        };
        app.world_mut()
            .resource_mut::<Lobby>()
            .add_bootstrap_with_context([(peer_id_from_socket(domain, seed), context)]);
    }
    app.insert_resource(AwaitingP2PStart);
    app.add_systems(Update, start_when_lobby_ready);
}

fn validate_settings(expected_players: u8) {
    assert!(
        (2..=MAX_P2P_PLAYERS).contains(&expected_players),
        "P2P players must be between 2 and {MAX_P2P_PLAYERS}"
    );
}

/// Start the deterministic session once the discovered roster is complete.
///
/// Every peer computes the same sorted roster from the same announces, so
/// gating on the roster (rather than a configured address list) starts all
/// peers with the same cohort.
fn start_when_lobby_ready(
    mut commands: Commands,
    settings: Res<P2PSettings>,
    lobby: Res<Lobby>,
    awaiting_start: Option<Res<AwaitingP2PStart>>,
) {
    if awaiting_start.is_none() {
        return;
    }
    let expected = usize::from(settings.expected_players);
    // The roster holds every member plus ourselves; all of them must be linked.
    if lobby.roster().len() != expected || lobby.connected_members().count() + 1 != expected {
        return;
    }

    tracing::info!(
        expected_players = expected,
        roster = ?lobby.roster(),
        "P2P lobby complete; starting session negotiation"
    );
    commands.remove_resource::<AwaitingP2PStart>();
    commands.trigger(P2PStart);
}
