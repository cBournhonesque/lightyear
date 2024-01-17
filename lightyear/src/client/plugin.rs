//! Defines the client bevy plugin
use std::ops::DerefMut;
use std::sync::Mutex;

use crate::client::connection::{replication_clean, ConnectionManager};
use crate::client::diagnostics::ClientDiagnosticsPlugin;
use bevy::prelude::*;
use bevy::time::common_conditions::on_timer;
use bevy::transform::TransformSystem;
use bevy::utils::Duration;

use crate::client::events::{ConnectEvent, DisconnectEvent, EntityDespawnEvent, EntitySpawnEvent};
use crate::client::input::InputPlugin;
use crate::client::interpolation::plugin::InterpolationPlugin;
use crate::client::prediction::plugin::{is_connected, is_in_rollback, PredictionPlugin};
use crate::client::prediction::Rollback;
use crate::client::resource::{Authentication, Client};
use crate::client::systems::{receive, send, sync_update};
use crate::connection::events::ConnectionEvents;
use crate::prelude::{ReplicationSet, TimeManager};
use crate::protocol::component::ComponentProtocol;
use crate::protocol::message::MessageProtocol;
use crate::protocol::Protocol;
use crate::shared::plugin::SharedPlugin;
use crate::shared::replication::systems::add_replication_send_systems;
use crate::shared::sets::{FixedUpdateSet, MainSet};
use crate::shared::time_manager::{is_ready_to_send, TimePlugin};
use crate::transport::io::Io;

use super::config::ClientConfig;

pub struct PluginConfig<P: Protocol> {
    client_config: ClientConfig,
    io: Io,
    protocol: P,
    auth: Authentication,
}

impl<P: Protocol> PluginConfig<P> {
    pub fn new(client_config: ClientConfig, io: Io, protocol: P, auth: Authentication) -> Self {
        PluginConfig {
            client_config,
            io,
            protocol,
            auth,
        }
    }
}

pub struct ClientPlugin<P: Protocol> {
    // we add Mutex<Option> so that we can get ownership of the inner from an immutable reference
    // in build()
    config: Mutex<Option<PluginConfig<P>>>,
}

impl<P: Protocol> ClientPlugin<P> {
    pub fn new(config: PluginConfig<P>) -> Self {
        Self {
            config: Mutex::new(Some(config)),
        }
    }
}

impl<P: Protocol> Plugin for ClientPlugin<P> {
    fn build(&self, app: &mut App) {
        let config = self.config.lock().unwrap().deref_mut().take().unwrap();

        let token = config
            .auth
            .get_token(config.client_config.netcode.client_timeout_secs)
            .expect("could not generate token");
        let token_bytes = token.try_into_bytes().unwrap();
        let netcode =
            crate::netcode::Client::with_config(&token_bytes, config.client_config.netcode.build())
                .expect("could not create netcode client");
        let fixed_timestep = config.client_config.shared.tick.tick_duration;
        let clean_interval = fixed_timestep * (i16::MAX as u32 / 3);

        add_replication_send_systems::<P, ConnectionManager<P>>(app);
        P::Components::add_per_component_replication_send_systems::<ConnectionManager<P>>(app);
        P::Components::add_events::<()>(app);
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
            .add_plugins(InputPlugin::<P>::default())
            .add_plugins(PredictionPlugin::<P>::new(config.client_config.prediction))
            .add_plugins(InterpolationPlugin::<P>::new(
                config.client_config.interpolation.clone(),
            ))
            .add_plugins(TimePlugin {
                send_interval: config.client_config.shared.client_send_interval,
            })
            .add_plugins(ClientDiagnosticsPlugin::<P>::default())
            // RESOURCES //
            .insert_resource(config.client_config.clone())
            .insert_resource(config.io)
            .insert_resource(netcode)
            .insert_resource(ConnectionManager::<P>::new(
                config.protocol.channel_registry(),
                config.client_config.sync,
                &config.client_config.ping,
                config.client_config.prediction.input_delay_ticks,
            ))
            .insert_resource(ConnectionEvents::<P>::new())
            .insert_resource(config.protocol)
            // SYSTEM SETS //
            .configure_sets(PreUpdate, (MainSet::Receive, MainSet::ReceiveFlush).chain())
            .configure_sets(
                FixedUpdate,
                (
                    FixedUpdateSet::TickUpdate,
                    FixedUpdateSet::Main,
                    FixedUpdateSet::MainFlush,
                )
                    .chain(),
            )
            // TODO: revisit the ordering of systems here. I believe all systems in ReplicationSet::All can run in parallel,
            //  but maybe that's not the case and we need to run them in a certain order
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
                        .in_set(ReplicationSet::All)
                        .after(TransformSystem::TransformPropagate),
                    (
                        ReplicationSet::SendEntityUpdates,
                        ReplicationSet::SendComponentUpdates,
                        // NOTE: SendDespawnsAndRemovals is not in MainSet::Send because we need to run them every frame
                        MainSet::SendPackets,
                    )
                        .in_set(MainSet::Send)
                        .after(TransformSystem::TransformPropagate),
                    // ReplicationSystems runs once per frame, so we cannot put it in the `Send` set
                    // which runs every send_interval
                    (ReplicationSet::All, MainSet::SendPackets).chain(),
                    // only replicate entities once client is connected
                    // TODO: should it be only when the client is synced? because before that the ticks might be incorrect!
                    ReplicationSet::All.run_if(is_connected),
                ),
            )
            .configure_sets(
                PostUpdate,
                // run sync before send because some send systems need to know if the client is synced
                (MainSet::Sync, MainSet::Send.run_if(is_ready_to_send)).chain(),
            )
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
            // TODO: update virtual time with Time<Real> so we have more accurate time at Send time.
            .add_systems(
                PostUpdate,
                (
                    send::<P>.in_set(MainSet::SendPackets),
                    sync_update::<P>.in_set(MainSet::Sync),
                ),
            )
            .add_systems(
                Last,
                replication_clean::<P>.run_if(on_timer(clean_interval)),
            );
    }
}
