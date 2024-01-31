//! Defines the server bevy plugin
use bevy::app::Last;
use std::ops::DerefMut;
use std::sync::Mutex;

use crate::_reexport::ShouldBeInterpolated;
use bevy::prelude::{
    apply_deferred, App, FixedUpdate, IntoSystemConfigs, IntoSystemSetConfigs,
    Plugin as PluginType, PostUpdate, PreUpdate,
};
use bevy::time::common_conditions::on_timer;

use crate::netcode::ClientId;
use crate::prelude::{ShouldBePredicted, TimeManager};
use crate::protocol::component::ComponentProtocol;
use crate::protocol::message::MessageProtocol;
use crate::protocol::Protocol;
use crate::server::connection::replication_clean;
use crate::server::connection::ConnectionManager;
use crate::server::events::{ConnectEvent, DisconnectEvent, EntityDespawnEvent, EntitySpawnEvent};
use crate::server::input::InputPlugin;
use crate::server::prediction::compute_hash;
use crate::server::resource::Server;
use crate::server::room::RoomPlugin;
use crate::server::systems::clear_events;
use crate::shared::plugin::SharedPlugin;
use crate::shared::replication::systems::add_replication_send_systems;
use crate::shared::sets::ReplicationSet;
use crate::shared::sets::{FixedUpdateSet, MainSet};
use crate::shared::time_manager::{is_ready_to_send, TimePlugin};
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
        let netserver = crate::netcode::Server::new(
            config.io.local_addr(),
            config.server_config.netcode.clone(),
        );

        let tick_duration = config.server_config.shared.tick.tick_duration;
        // TODO: have better constants for clean_interval?
        let clean_interval = tick_duration * (i16::MAX as u32 / 3);

        // TODO: maybe put those 2 in a ReplicationPlugin?
        add_replication_send_systems::<P, ConnectionManager<P>>(app);
        P::Components::add_per_component_replication_send_systems::<ConnectionManager<P>>(app);
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
            .add_plugins(TimePlugin {
                send_interval: config.server_config.shared.server_send_interval,
            })
            // RESOURCES //
            .insert_resource(config.server_config.clone())
            .insert_resource(config.io)
            .insert_resource(netserver)
            .insert_resource(ConnectionManager::<P>::new(
                config.protocol.channel_registry().clone(),
                config.server_config.packet,
                config.server_config.ping,
            ))
            .insert_resource(config.protocol)
            // .insert_resource(server)
            // SYSTEM SETS //
            .configure_sets(
                PreUpdate,
                (
                    MainSet::Receive,
                    MainSet::ReceiveFlush,
                    MainSet::ClientReplication,
                    MainSet::ClientReplicationFlush,
                )
                    .chain(),
            )
            // NOTE: it's ok to run the replication systems less frequently than every frame
            //  because bevy's change detection detects changes since the last time the system ran (not since the last frame)
            .configure_sets(
                PostUpdate,
                (
                    (
                        // we need to set the value of hash before replicating the component
                        ReplicationSet::SetPreSpawnedHash
                            .before(ReplicationSet::SendComponentUpdates),
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
                    // some replication systems have to run once per frame, so we cannot put them in the `Send` set
                    // which runs every send_interval
                    (ReplicationSet::All, MainSet::SendPackets).chain(),
                ),
            )
            .configure_sets(PostUpdate, MainSet::ClearEvents)
            .configure_sets(PostUpdate, MainSet::Send.run_if(is_ready_to_send))
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
            .add_systems(
                PostUpdate,
                (
                    compute_hash::<P>.in_set(ReplicationSet::SetPreSpawnedHash),
                    send::<P>.in_set(MainSet::SendPackets),
                    clear_events::<P>.in_set(MainSet::ClearEvents),
                ),
            )
            .add_systems(
                Last,
                replication_clean::<P>.run_if(on_timer(clean_interval)),
            );
    }
}
