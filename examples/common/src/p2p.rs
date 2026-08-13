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
}

impl P2PSettings {
    pub fn peer_ids(&self) -> Range<u8> {
        0..self.player_count
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
    });

    #[cfg(any(feature = "gui2d", feature = "gui3d"))]
    if !_headless {
        app.add_plugins(ExampleClientRendererPlugin::new(format!(
            "P2P Peer {peer_id}"
        )));
    }
}

/// Spawn one directed raw UDP Link for every other member of the fixed roster.
pub(crate) fn spawn_connections(
    app: &mut App,
    conditioner: &LinkConditionerConfig,
    peer_id: u8,
    player_count: u8,
    base_port: u16,
) {
    validate_roster(peer_id, player_count);
    let local_id = PeerId::Entity(u64::from(peer_id));
    for remote_peer_id in 0..player_count {
        if remote_peer_id == peer_id {
            continue;
        }
        let local_addr = peer_addr(base_port, peer_id, remote_peer_id);
        let remote_addr = peer_addr(base_port, remote_peer_id, peer_id);
        app.world_mut().spawn((
            P2P::default(),
            RawClient,
            LocalId(local_id),
            RemoteId(PeerId::Entity(u64::from(remote_peer_id))),
            PingManager::default(),
            LocalAddr(local_addr),
            PeerAddr(remote_addr),
            UdpIo::default(),
            Link::default().with_conditioner(Some(RecvLinkConditioner::new(conditioner.clone()))),
            Name::new(format!("P2P Link {peer_id} -> {remote_peer_id}")),
        ));
    }
    app.add_systems(Startup, connect_and_start);
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

fn connect_and_start(mut commands: Commands, links: Query<Entity, With<P2P>>) {
    for entity in &links {
        commands.trigger(Connect { entity });
    }
    commands.trigger(P2PStart);
}
