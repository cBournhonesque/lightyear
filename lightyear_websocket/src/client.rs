use crate::WebSocketError;
use aeronet_io::connection::PeerAddr;
use aeronet_websocket::client::{ClientConfig, WebSocketClient};
use alloc::format;
use bevy_app::{App, Plugin};
use bevy_ecs::{
    error::Result,
    prelude::{Commands, Component, Entity, EntityCommand, Name, Query, Trigger, Without, World},
};
use lightyear_aeronet::{AeronetLinkOf, AeronetPlugin};
use lightyear_link::{Link, LinkStart, Linked, Linking};

pub struct WebSocketClientPlugin;

impl Plugin for WebSocketClientPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<AeronetPlugin>() {
            app.add_plugins(AeronetPlugin);
        }
        app.add_plugins(aeronet_websocket::client::WebSocketClientPlugin);
        app.add_observer(Self::link);
    }
}

/// WebSocket session implementation which acts as a dedicated client,
/// connecting to a target endpoint.
///
/// The [`PeerAddr`] component will be used to find the server_addr.
///
/// Use [`WebSocketClient::connect`] to start a connection.
#[derive(Component)]
#[require(Link)]
pub struct WebSocketClientIo {
    pub config: ClientConfig,
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
            let server_addr = peer_addr.ok_or(WebSocketError::PeerAddrMissing)?.0;
            let config = client.config.clone();
            commands.queue(move |world: &mut World| -> Result {
                let target = format!("wss://{server_addr}");
                let entity_mut =
                    world.spawn((AeronetLinkOf(entity), Name::from("WebSocketClient")));
                WebSocketClient::connect(config, target).apply(entity_mut);
                Ok(())
            });
        }
        Ok(())
    }
}
