//! Glue between [`lightyear_p2p::Lobby`] and UDP links.
//!
//! The lobby discovers peers and emits [`DialPeer`](lightyear_p2p::DialPeer), but never says *where*
//! a peer is: that is the transport's business. For UDP this module answers it with one rule — **a
//! peer's address is its identity**.
//!
//! # Why an endpoint
//!
//! A [`PeerId::Raw`](lightyear_core::id::PeerId::Raw) is a [`SocketAddr`](core::net::SocketAddr), so
//! the announce that carries peer identities carries their addresses too, and dialing a peer is
//! sending to that address. Nothing else has to be supplied: the application never maps peers to
//! addresses.
//!
//! That only holds if a peer has exactly *one* address that everyone can reach, which is what
//! [`UdpEndpoint`](crate::endpoint::UdpEndpoint) provides. It owns a single socket and fans out to one child [`Link`] per remote
//! address, so:
//!
//! - every peer reaches this one at the same address, whether it dialed us or we dialed it;
//! - the source address of an inbound datagram identifies the sender, so a peer that dialed us needs
//!   no introduction;
//! - seeding is one-way: whoever is dialed learns the dialer from the datagram and can reply on the
//!   link it already has.
//!
//! # What the application supplies
//!
//! Only two things, neither of them per-peer:
//!
//! 1. **Where to bind**, by spawning an [`UdpEndpoint`](crate::endpoint::UdpEndpoint) with a
//!    [`LocalAddr`](aeronet_io::connection::LocalAddr). Bind port `0` to let
//!    the OS choose; the endpoint updates [`LocalAddr`] with the address it actually got.
//! 2. **The seeds**, by handing the lobby the `PeerId::Raw` of the peers it should dial first. Every
//!    other peer is discovered through announces and dialed with no configuration at all.
//!
//! ```no_run
//! # use core::net::{Ipv4Addr, SocketAddr};
//! # use lightyear_udp::endpoint::UdpEndpoint;
//! # use lightyear_udp::lobby::UdpLobbyPlugin;
//! # use aeronet_io::connection::LocalAddr;
//! # use bevy_app::App;
//! fn setup(app: &mut App) {
//!     app.add_plugins(UdpLobbyPlugin);
//!     // Give the peer a socket. Any address the peers can reach will do.
//!     let local = SocketAddr::new(Ipv4Addr::LOCALHOST.into(), 0);
//!     app.world_mut()
//!         .spawn((UdpEndpoint::default(), LocalAddr(local)));
//! }
//! ```
//!
//! # Liveness
//!
//! UDP has no handshake, so a Link is [`Linked`] and `Connected` from the moment it is dialed: that
//! means the socket is usable, not that the peer is listening. Readiness has to be judged from
//! traffic the peer sends back — the examples gate on ping samples for this reason — rather than
//! from a Link merely existing.
//!
//! # Foreign peers
//!
//! A peer that reaches this endpoint but belongs to another lobby is recorded by the lobby as
//! foreign and never added to the roster. Its [`Link`] still exists, and a Link carrying [`P2P`](lightyear_connection::p2p::P2P)
//! counts towards the start barrier, so an application that starts a session while a foreign peer is
//! attached should unlink it first.

use bevy_app::{App, Plugin};
use bevy_ecs::prelude::*;
use lightyear_core::id::PeerId;
use lightyear_link::prelude::{Link, LinkOf, LinkStart, Linked};
use lightyear_p2p::DialPeer;
use tracing::{debug, warn};

use aeronet_io::connection::PeerAddr;

use crate::endpoint::{UdpEndpoint, UdpEndpointPlugin, UdpLinkOfIO};

/// Dials UDP Links for a [`Lobby`](lightyear_p2p::Lobby) through a [`UdpEndpoint`].
pub struct UdpLobbyPlugin;

/// Identifies endpoints whose children need P2P roles before admission.
#[derive(Component, Default)]
pub(crate) struct UdpLobbyEndpoint;

impl Plugin for UdpLobbyPlugin {
    fn build(&self, app: &mut App) {
        app.register_required_components::<UdpEndpoint, UdpLobbyEndpoint>();
        if !app.is_plugin_added::<UdpEndpointPlugin>() {
            app.add_plugins(UdpEndpointPlugin);
        }
        app.add_observer(on_endpoint_added);
        app.add_observer(on_dial);
    }
}

/// Binds the endpoint's socket as soon as it exists.
fn on_endpoint_added(trigger: On<Add, UdpEndpoint>, mut commands: Commands) {
    commands.trigger(LinkStart {
        entity: trigger.entity,
    });
}

/// Opens a Link to the peer the lobby asked for.
///
/// The peer's address is its identity, so nothing else is needed to reach it. The link is registered
/// with the endpoint so that the peer's reply is delivered to it instead of spawning a second link
/// for the same address.
fn on_dial(
    trigger: On<DialPeer>,
    endpoints: Query<Entity, With<UdpEndpoint>>,
    peer_links: Query<(&LinkOf, &PeerAddr), With<UdpLinkOfIO>>,
    mut ios: Query<&mut UdpEndpoint>,
    mut commands: Commands,
) {
    let peer = trigger.peer;
    let PeerId::Raw(address) = peer else {
        warn!(
            ?peer,
            "a UDP peer must be identified by PeerId::Raw(address); this peer cannot be dialed"
        );
        return;
    };

    let Ok(endpoint) = endpoints.single() else {
        warn!(
            ?peer,
            "no UdpEndpoint exists, so no Link can be dialed; spawn an UdpEndpoint with a LocalAddr"
        );
        return;
    };

    // Reconnecting to a peer we already have a link to would put two links on one address, and the
    // endpoint would deliver the peer's datagrams to only one of them.
    if peer_links
        .iter()
        .any(|(link_of, peer_addr)| link_of.endpoint == endpoint && peer_addr.0 == address)
    {
        debug!(
            ?peer,
            "a Link to this peer already exists; not dialing again"
        );
        return;
    }

    let link = commands
        .spawn((
            LinkOf { endpoint },
            Link::default(),
            // The endpoint's socket is already bound, so the link can send immediately.
            Linked,
            PeerAddr(address),
            UdpLinkOfIO,
        ))
        .id();
    if let Ok(mut io) = ios.get_mut(endpoint) {
        io.register_link(address, link);
    }
    debug!(?peer, %address, ?link, "lobby dialed a UDP peer");
}
