//! Defines the server bevy plugin
use std::ops::DerefMut;
use std::sync::Mutex;

use bevy::prelude::{
    apply_deferred, App, FixedUpdate, IntoSystemConfigs, IntoSystemSetConfigs,
    Plugin as PluginType, PostUpdate, PreUpdate,
};

use crate::netcode::ClientId;
use crate::protocol::component::ComponentProtocol;
use crate::protocol::message::MessageProtocol;
use crate::protocol::Protocol;
use crate::server::events::{ConnectEvent, DisconnectEvent, EntityDespawnEvent, EntitySpawnEvent};
use crate::server::input::InputPlugin;
use crate::server::resource::Server;
use crate::server::room::RoomPlugin;
use crate::server::systems::{clear_events, is_ready_to_send};
use crate::shared::plugin::SharedPlugin;
use crate::shared::replication::systems::add_replication_send_systems;
use crate::shared::sets::ReplicationSet;
use crate::shared::sets::{FixedUpdateSet, MainSet};
use crate::shared::systems::tick::increment_tick;
use crate::transport::io::Io;

use super::config::ServerConfig;
use super::systems::{receive, send};

pub struct PluginConfig<P: Protocol> {
    server_config: ServerConfig,
    io: Io,
    protocol: P,
}

// TODO: put all this in ClientConfig?
impl<P: Protocol> PluginConfig<P> {
    pub fn new(server_config: ServerConfig, io: Io, protocol: P) -> Self {
        PluginConfig {
            server_config,
            io,
            protocol,
        }
    }
}

pub struct ServerPlugin<P: Protocol> {
    // we add Mutex<Option> so that we can get ownership of the inner from an immutable reference
    // in build()
    config: Mutex<Option<PluginConfig<P>>>,
}

impl<P: Protocol> ServerPlugin<P> {
    pub fn new(config: PluginConfig<P>) -> Self {
        Self {
            config: Mutex::new(Some(config)),
        }
    }
}

impl<P: Protocol> PluginType for ServerPlugin<P> {
    fn build(&self, app: &mut App) {
        let config = self.config.lock().unwrap().deref_mut().take().unwrap();
        let server = Server::new(config.server_config.clone(), config.io, config.protocol);

        // TODO: maybe put those 2 in a ReplicationPlugin?
        add_replication_send_systems::<P, Server<P>>(app);
        P::Components::add_per_component_replication_send_systems::<Server<P>>(app);
        P::Components::add_events::<ClientId>(app);

        P::Message::add_events::<ClientId>(app);

        app
            // PLUGINS
            .add_plugins(SharedPlugin {
                // TODO: move shared config out of server_config
                config: config.server_config.shared.clone(),
            })
            .add_plugins(InputPlugin::<P>::default())
            .add_plugins(RoomPlugin::<P>::default())
            // RESOURCES //
            .insert_resource(server)
            // SYSTEM SETS //
            .configure_sets(PreUpdate, (MainSet::Receive, MainSet::ReceiveFlush).chain())
            // NOTE: it's ok to run the replication systems less frequently than every frame
            //  because bevy's change detection detects changes since the last time the system ran (not since the last frame)
            .configure_sets(
                PostUpdate,
                (
                    (
                        ReplicationSet::SendEntityUpdates,
                        ReplicationSet::SendComponentUpdates,
                        ReplicationSet::SendDespawnsAndRemovals,
                    )
                        .in_set(ReplicationSet::All),
                    (
                        ReplicationSet::SendEntityUpdates,
                        ReplicationSet::SendComponentUpdates,
                        MainSet::SendPackets,
                    )
                        .in_set(MainSet::Send),
                    // ReplicationSystems runs once per frame, so we cannot put it in the `Send` set
                    // which runs every send_interval
                    (ReplicationSet::All, MainSet::SendPackets).chain(),
                ),
            )
            .configure_sets(PostUpdate, MainSet::ClearEvents)
            .configure_sets(PostUpdate, MainSet::Send.run_if(is_ready_to_send::<P>))
            // EVENTS //
            .add_event::<ConnectEvent>()
            .add_event::<DisconnectEvent>()
            .add_event::<EntitySpawnEvent>()
            .add_event::<EntityDespawnEvent>()
            // SYSTEMS //
            .add_systems(
                PreUpdate,
                (
                    receive::<P>.in_set(MainSet::Receive),
                    apply_deferred.in_set(MainSet::ReceiveFlush),
                ),
            )
            // TODO: a bit of a code-smell that i have to run this here instead of in the shared plugin
            //  maybe TickManager should be a separate resource not contained in Client/Server?
            .add_systems(
                FixedUpdate,
                increment_tick::<Server<P>>.in_set(FixedUpdateSet::TickUpdate),
            )
            .add_systems(
                PostUpdate,
                (
                    send::<P>.in_set(MainSet::SendPackets),
                    clear_events::<P>.in_set(MainSet::ClearEvents),
                ),
            );
    }
}
