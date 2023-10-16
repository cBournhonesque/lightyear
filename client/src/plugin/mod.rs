mod events;

use crate::config::ClientConfig;
use crate::Client;
use bevy_app::{App, Plugin as PluginType};
use lightyear_shared::netcode::ConnectToken;
use lightyear_shared::{Io, Protocol};
use std::sync::Mutex;

pub struct PluginConfig<P: Protocol> {
    client_config: ClientConfig,
    protocol: P,
    io: Io,
    token: ConnectToken,
}

// TODO: put all this in ClientConfig?
impl<P: Protocol> PluginConfig<P> {
    pub fn new(client_config: ClientConfig, protocol: P, io: Io, token: ConnectToken) -> Self {
        PluginConfig {
            client_config,
            protocol,
            io,
            token,
        }
    }
}
pub struct Plugin<P: Protocol> {
    config: PluginConfig<P>,
}

impl<P: Protocol> Plugin<P> {
    pub fn new(config: PluginConfig<P>) -> Self {
        Self { config }
    }
}

impl<P: Protocol> PluginType for Plugin<P> {
    fn build(&self, app: &mut App) {
        // let io = std::mem::take(&mut self.config.io);
        // let client = Client::new(io, config.token, config.protocol);

        // app
        // RESOURCES //
        // .insert_resource(client);
        // EVENTS //
        // SYSTEM SETS //
        // SYSTEMS //
    }
}
