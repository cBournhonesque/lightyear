use std::ops::DerefMut;
use std::sync::Mutex;

use bevy::prelude::{App, IntoSystemConfigs, Plugin as PluginType, PostUpdate, PreUpdate};

use lightyear_shared::{MessageProtocol, Protocol};

use crate::client::Authentication;
use crate::config::ClientConfig;
use crate::plugin::sets::ClientSet;
use crate::plugin::systems::{receive, send};
use crate::Client;

mod events;
mod sets;
mod systems;

pub struct PluginConfig<P: Protocol> {
    client_config: ClientConfig,
    protocol: P,
    auth: Authentication,
}

// TODO: put all this in ClientConfig?
impl<P: Protocol> PluginConfig<P> {
    pub fn new(client_config: ClientConfig, protocol: P, auth: Authentication) -> Self {
        PluginConfig {
            client_config,
            protocol,
            auth,
        }
    }
}
pub struct Plugin<P: Protocol> {
    // we add Mutex<Option> so that we can get ownership of the inner from an immutable reference
    // in build()
    config: Mutex<Option<PluginConfig<P>>>,
}

impl<P: Protocol> Plugin<P> {
    pub fn new(config: PluginConfig<P>) -> Self {
        Self {
            config: Mutex::new(Some(config)),
        }
    }
}

impl<P: Protocol> PluginType for Plugin<P> {
    fn build(&self, app: &mut App) {
        let config = self.config.lock().unwrap().deref_mut().take().unwrap();
        let client = Client::new(config.client_config, config.auth, config.protocol);

        // TODO: it's annoying to have to keep that () around...
        //  revisit this.. amybe the into_iter_messages returns directly an object that
        //  can be created from Ctx and Message
        //  For Server it's the MessageEvent<M, ClientId>
        //  For Client it's MessageEvent<M> directly
        P::Message::add_events::<()>(app);

        app
            // RESOURCES //
            .insert_resource(client)
            // SYSTEM SETS //
            .configure_set(PreUpdate, ClientSet::Receive)
            .configure_set(PostUpdate, ClientSet::Send)
            // EVENTS //
            // SYSTEMS //
            .add_systems(PreUpdate, receive::<P>.in_set(ClientSet::Receive))
            .add_systems(PostUpdate, send::<P>.in_set(ClientSet::Send));
    }
}
