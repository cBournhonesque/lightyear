//! Client-side WebSocket transport integration.
//!
//! [`WebSocketClientIo`](crate::client::WebSocketClientIo) is a Lightyear link component that
//! spawns an Aeronet WebSocket client entity when [`LinkStart`](lightyear_link::LinkStart) is
//! triggered. `lightyear_aeronet` then mirrors Aeronet session state and moves payloads between the
//! Aeronet session and the Lightyear [`Link`](lightyear_link::Link).

use crate::{WebSocketLocalAddr, WebSocketPeerAddr};
use aeronet_io::connection::{LocalAddr, PeerAddr};
use aeronet_websocket::client::{ClientConfig, WebSocketClient};
use alloc::{format, string::String};
use bevy_app::{App, Plugin};
use bevy_ecs::prelude::*;
use core::net::SocketAddr;
use lightyear_aeronet::{AeronetLinkOf, AeronetPlugin};
use lightyear_connection::admission::{DefaultConnectionPlugin, apply_default_ids};
use lightyear_connection::p2p::P2P;
use lightyear_core::id::{LocalId, PeerId, RemoteId};
use lightyear_link::{Link, LinkStart, Linked, Linking};

/// Plugin that starts WebSocket client sessions for [`WebSocketClientIo`] link entities.
///
/// The plugin observes [`LinkStart`] to spawn and connect the underlying Aeronet client entity.
pub struct WebSocketClientPlugin;

/// URL scheme used when constructing a WebSocket URL.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum WebSocketScheme {
    /// Unencrypted WebSocket (ws://)
    Plain,
    /// Encrypted WebSocket (wss://) - default
    #[default]
    Secure,
}

impl WebSocketScheme {
    /// Returns the URL scheme string used by `aeronet_websocket`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Plain => "ws",
            Self::Secure => "wss",
        }
    }
}

impl Plugin for WebSocketClientPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<AeronetPlugin>() {
            app.add_plugins(AeronetPlugin);
        }
        if !app.is_plugin_added::<DefaultConnectionPlugin>() {
            app.add_plugins(DefaultConnectionPlugin);
        }
        app.add_plugins(aeronet_websocket::client::WebSocketClientPlugin);
        app.add_observer(Self::link);
        app.add_observer(Self::on_linked);
        app.add_observer(Self::on_addresses);
    }
}

/// Lightyear component for a WebSocket client transport.
///
/// Insert this on the entity that owns the client-side [`Link`]. When [`LinkStart`] is triggered,
/// [`WebSocketClientPlugin`] spawns an Aeronet WebSocket client entity related back to this link.
/// A [`Client`](lightyear_connection::client::Client) role is added automatically. On browsers,
/// which hide socket addresses, default admission uses the entity as a local-only debug identity.
#[derive(Component)]
#[require(Link, lightyear_connection::client::Client)]
pub struct WebSocketClientIo {
    /// Aeronet WebSocket client configuration.
    pub config: ClientConfig,
    /// Full WebSocket URL, including the original hostname, path and query.
    pub target: String,
}

impl WebSocketClientIo {
    /// Creates a client transport that connects to a full WebSocket URL.
    ///
    /// Hostnames, paths and queries are passed unchanged to the transport.
    pub fn from_url(config: ClientConfig, url: impl Into<String>) -> Self {
        Self {
            config,
            target: url.into(),
        }
    }

    /// Creates a client transport for a socket address.
    pub fn from_addr(config: ClientConfig, scheme: WebSocketScheme, address: SocketAddr) -> Self {
        Self::from_url(config, format!("{}://{address}", scheme.as_str()))
    }

    /// Creates a client transport for a hostname without resolving DNS in advance.
    pub fn from_host(config: ClientConfig, scheme: WebSocketScheme, host: &str, port: u16) -> Self {
        let host = host.trim_start_matches('[').trim_end_matches(']');
        let target = if host.contains(':') {
            format!("{}://[{host}]:{port}", scheme.as_str())
        } else {
            format!("{}://{host}:{port}", scheme.as_str())
        };
        Self::from_url(config, target)
    }
}

impl WebSocketClientPlugin {
    fn on_addresses(
        trigger: On<Insert, (LocalAddr, PeerAddr, AeronetLinkOf, WebSocketClient)>,
        query: Query<
            (&AeronetLinkOf, Option<&LocalAddr>, Option<&PeerAddr>),
            With<WebSocketClient>,
        >,
        mut commands: Commands,
    ) {
        if let Ok((owner, local, peer)) = query.get(trigger.entity) {
            let mut link = commands.entity(owner.0);
            if let Some(local) = local {
                link.insert(WebSocketLocalAddr(local.0));
            }
            if let Some(peer) = peer {
                link.insert(WebSocketPeerAddr(peer.0));
            }
        }
    }

    fn on_linked(
        trigger: On<Add, (Linked, WebSocketLocalAddr, WebSocketPeerAddr)>,
        query: Query<
            (Option<&WebSocketLocalAddr>, Option<&WebSocketPeerAddr>),
            (With<WebSocketClientIo>, With<Linked>, Without<P2P>),
        >,
        mut commands: Commands,
    ) {
        let Ok((local_addr, peer_addr)) = query.get(trigger.entity) else {
            return;
        };
        let entity = trigger.entity;
        #[cfg(not(target_family = "wasm"))]
        let (local, remote) = {
            let (Some(local_addr), Some(peer_addr)) = (local_addr, peer_addr) else {
                return;
            };
            (PeerId::Raw(local_addr.0), PeerId::Raw(peer_addr.0))
        };
        // Browsers do not expose socket addresses. These are local debug routing labels only.
        #[cfg(target_family = "wasm")]
        let (local, remote) = {
            let _ = (local_addr, peer_addr);
            (PeerId::Entity(entity.to_bits()), PeerId::Server)
        };
        commands.queue(move |world: &mut World| {
            apply_default_ids(world, entity, LocalId(local), RemoteId(remote));
        });
    }

    fn link(
        trigger: On<LinkStart>,
        query: Query<(Entity, &WebSocketClientIo), (Without<Linking>, Without<Linked>)>,
        mut commands: Commands,
    ) -> Result {
        if let Ok((entity, client)) = query.get(trigger.entity) {
            let target = client.target.clone();

            let config = client.config.clone();
            commands.queue(move |world: &mut World| -> Result {
                let entity_mut =
                    world.spawn((AeronetLinkOf(entity), Name::from("WebSocketClient")));
                WebSocketClient::connect(config, target).apply(entity_mut);
                Ok(())
            });
        }
        Ok(())
    }
}

#[cfg(all(test, not(target_family = "wasm")))]
mod tests {
    use super::*;

    #[test]
    fn host_constructor_brackets_ipv6_once() {
        for host in ["::1", "[::1]"] {
            let client = WebSocketClientIo::from_host(
                ClientConfig::builder().with_no_encryption(),
                WebSocketScheme::Plain,
                host,
                4100,
            );
            assert_eq!(client.target, "ws://[::1]:4100");
        }
    }
}
