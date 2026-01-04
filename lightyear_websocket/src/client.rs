use crate::WebSocketError;
use aeronet_io::connection::PeerAddr;
use aeronet_websocket::client::{ClientConfig, WebSocketClient};
use alloc::{format, string::String};
use bevy_app::{App, Plugin};
use bevy_ecs::prelude::*;
use lightyear_aeronet::{AeronetLinkOf, AeronetPlugin};
use lightyear_link::{Link, LinkStart, Linked, Linking};

pub struct WebSocketClientPlugin;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum WebSocketScheme {
    /// Unencrypted WebSocket (ws://)
    Plain,
    /// Encrypted WebSocket (wss://) - default
    #[default]
    Secure,
}

impl WebSocketScheme {
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
        app.add_plugins(aeronet_websocket::client::WebSocketClientPlugin);
        app.add_observer(Self::link);
    }
}

/// WebSocket client that connects to a target endpoint.
#[derive(Component)]
#[require(Link)]
pub struct WebSocketClientIo {
    pub config: ClientConfig,
    pub target: WebSocketTarget,
}

#[derive(Debug, Clone)]
pub enum WebSocketTarget {
    /// Connect using a full URL (e.g., "wss://example.com:443").
    Url(String),
    /// Construct URL from scheme and [`PeerAddr`] component.
    Addr(WebSocketScheme),
}

impl WebSocketClientIo {
    /// Connect to a full URL.
    pub fn from_url(config: ClientConfig, url: impl Into<String>) -> Self {
        Self { config, target: WebSocketTarget::Url(url.into()) }
    }

    /// Construct URL from scheme and [`PeerAddr`].
    pub fn from_addr(config: ClientConfig, scheme: WebSocketScheme) -> Self {
        Self { config, target: WebSocketTarget::Addr(scheme) }
    }
}

impl WebSocketClientPlugin {
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
