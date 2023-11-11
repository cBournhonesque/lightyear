use std::ops::DerefMut;
use std::sync::Mutex;

use bevy::prelude::{
    App, Fixed, FixedUpdate, IntoSystemConfigs, Plugin as PluginType, PostUpdate, PreUpdate, Time,
};

use lightyear_shared::plugin::systems::tick::increment_tick;
use lightyear_shared::{
    ConnectEvent, DisconnectEvent, EntitySpawnEvent, MessageProtocol, Protocol, ReplicationData,
    SharedPlugin,
};

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
        let client = Client::new(config.client_config.clone(), config.auth, config.protocol);
        let fixed_timestep = config.client_config.tick.tick_duration.clone();

        // TODO: it's annoying to have to keep that () around...
        //  revisit this.. maybe the into_iter_messages returns directly an object that
        //  can be created from Ctx and Message
        //  For Server it's the MessageEvent<M, ClientId>
        //  For Client it's MessageEvent<M> directly
        P::Message::add_events::<()>(app);

        app
            // PLUGINS //
            .add_plugins(SharedPlugin {
                config: config.client_config.shared.clone(),
            })
            // RESOURCES //
            .insert_resource(client)
            .init_resource::<ReplicationData>()
            // SYSTEM SETS //
            .configure_sets(PreUpdate, ClientSet::Receive)
            .configure_sets(PostUpdate, ClientSet::Send)
            // EVENTS //
            .add_event::<ConnectEvent>()
            .add_event::<DisconnectEvent>()
            .add_event::<EntitySpawnEvent>()
            // SYSTEMS //
            .add_systems(PreUpdate, receive::<P>.in_set(ClientSet::Receive))
            // TODO: a bit of a code-smell that i have to run this here instead of in the shared plugin
            //  maybe TickManager should be a separate resource not contained in Client/Server?
            //  and runs Update in PreUpdate before the client/server systems
            .add_systems(FixedUpdate, increment_tick::<Client<P>>)
            .add_systems(PostUpdate, send::<P>.in_set(ClientSet::Send));
    }
}
