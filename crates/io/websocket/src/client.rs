//! Client-side WebSocket transport integration.
//!
//! [`WebSocketClientIo`](crate::client::WebSocketClientIo) is a Lightyear link component that
//! spawns an Aeronet WebSocket client entity when [`LinkStart`](lightyear_link::LinkStart) is
//! triggered. `lightyear_aeronet` then mirrors Aeronet session state and moves payloads between the
//! Aeronet session and the Lightyear [`Link`](lightyear_link::Link).

use crate::WebSocketError;
use aeronet_io::connection::{LocalAddr, PeerAddr};
use aeronet_websocket::client::{ClientConfig, WebSocketClient};
use alloc::{format, string::String};
use bevy_app::{App, Plugin};
use bevy_ecs::prelude::*;
use lightyear_aeronet::{AeronetLinkOf, AeronetPlugin};
use lightyear_connection::admission::{DefaultConnectionPlugin, apply_default_ids};
use lightyear_connection::p2p::P2P;
use lightyear_core::id::{LocalId, PeerId, RemoteId};
use lightyear_link::{Link, LinkStart, Linked, Linking};

/// Plugin that starts WebSocket client sessions for [`WebSocketClientIo`] link entities.
///
/// The plugin observes [`LinkStart`] to spawn and connect the underlying Aeronet client entity.
pub struct WebSocketClientPlugin;

/// URL scheme used when constructing a WebSocket URL from [`PeerAddr`].
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
    /// URL or address-derived target to connect to.
    pub target: WebSocketTarget,
}

/// Target endpoint for [`WebSocketClientIo`].
#[derive(Debug, Clone)]
pub enum WebSocketTarget {
    /// Connect using a full URL (e.g., "wss://example.com:443").
    Url(String),
    /// Construct URL from scheme and [`PeerAddr`] component.
    Addr(WebSocketScheme),
}

impl WebSocketClientIo {
    /// Creates a client transport that connects to a full WebSocket URL.
    ///
    /// Use this when the target cannot be represented by the entity's [`PeerAddr`], for example
    /// when a path, DNS name, proxy endpoint, or explicit scheme must be included.
    pub fn from_url(config: ClientConfig, url: impl Into<String>) -> Self {
        Self {
            config,
            target: WebSocketTarget::Url(url.into()),
        }
    }

    /// Creates a client transport that constructs its URL from [`PeerAddr`].
    ///
    /// The link entity must have [`PeerAddr`] when [`LinkStart`] is processed. The final URL is
    /// `<scheme>://<peer_addr>`, where `scheme` is provided by [`WebSocketScheme::as_str`].
    pub fn from_addr(config: ClientConfig, scheme: WebSocketScheme) -> Self {
        Self {
            config,
            target: WebSocketTarget::Addr(scheme),
        }
    }
}

impl WebSocketClientPlugin {
    fn on_linked(
        trigger: On<Add, Linked>,
        query: Query<
            (Option<&LocalAddr>, Option<&PeerAddr>),
            (With<WebSocketClientIo>, Without<P2P>),
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
        query: Query<
            (Entity, &WebSocketClientIo, Option<&PeerAddr>),
            (Without<Linking>, Without<Linked>),
        >,
        mut commands: Commands,
    ) -> Result {
        if let Ok((entity, client, peer_addr)) = query.get(trigger.entity) {
            let target = match &client.target {
                WebSocketTarget::Url(url) => url.clone(),
                WebSocketTarget::Addr(scheme) => {
                    let server_addr = peer_addr.ok_or(WebSocketError::PeerAddrMissing)?.0;
                    format!("{}://{server_addr}", scheme.as_str())
                }
            };

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
