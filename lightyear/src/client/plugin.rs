//! Defines the client bevy plugin
use std::ops::DerefMut;
use std::sync::Mutex;

use bevy::prelude::IntoSystemSetConfigs;
use bevy::prelude::{
    apply_deferred, not, resource_exists, App, Condition, FixedUpdate, IntoSystemConfigs,
    Plugin as PluginType, PostUpdate, PreUpdate,
};
use bevy::transform::TransformSystem;

use crate::client::events::{ConnectEvent, DisconnectEvent, EntityDespawnEvent, EntitySpawnEvent};
use crate::client::input::InputPlugin;
use crate::client::interpolation::plugin::InterpolationPlugin;
use crate::client::prediction::plugin::{is_in_rollback, PredictionPlugin};
use crate::client::prediction::{clean_prespawned_entity, Rollback};
use crate::client::resource::{Authentication, Client};
use crate::client::systems::{is_ready_to_send, receive, send, sync_update};
use crate::prelude::ReplicationSet;
use crate::protocol::component::ComponentProtocol;
use crate::protocol::message::MessageProtocol;
use crate::protocol::Protocol;
use crate::shared::plugin::SharedPlugin;
use crate::shared::replication::systems::add_replication_send_systems;
use crate::shared::sets::{FixedUpdateSet, MainSet};
use crate::shared::systems::tick::increment_tick;
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

impl<P: Protocol> PluginType for ClientPlugin<P> {
    fn build(&self, app: &mut App) {
        let config = self.config.lock().unwrap().deref_mut().take().unwrap();
        let client = Client::new(
            config.client_config.clone(),
            config.io,
            config.auth,
            config.protocol,
        );
        let fixed_timestep = config.client_config.shared.tick.tick_duration;

        add_replication_send_systems::<P, Client<P>>(app);
        P::Components::add_per_component_replication_send_systems::<Client<P>>(app);
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
            // RESOURCES //
            .insert_resource(client)
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
                        MainSet::SendPackets,
                    )
                        .in_set(MainSet::Send)
                        .after(TransformSystem::TransformPropagate),
                    // ReplicationSystems runs once per frame, so we cannot put it in the `Send` set
                    // which runs every send_interval
                    (ReplicationSet::All, MainSet::SendPackets).chain(),
                ),
            )
            .configure_sets(
                PostUpdate,
                // run sync before send because some send systems need to know if the client is synced
                (MainSet::Sync, MainSet::Send.run_if(is_ready_to_send::<P>)).chain(),
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
                    (receive::<P>).in_set(MainSet::Receive),
                    apply_deferred.in_set(MainSet::ReceiveFlush),
                ),
            )
            // TODO: a bit of a code-smell that i have to run this here instead of in the shared plugin
            //  maybe TickManager should be a separate resource not contained in Client/Server?
            //  and runs Update in PreUpdate before the client/server systems
            .add_systems(
                FixedUpdate,
                (
                    increment_tick::<Client<P>>
                        .in_set(FixedUpdateSet::TickUpdate)
                        // run if there is no rollback resource, or if we are not in rollback
                        .run_if((not(resource_exists::<Rollback>())).or_else(not(is_in_rollback))),
                    apply_deferred.in_set(FixedUpdateSet::MainFlush),
                ),
            )
            // TODO: update virtual time with Time<Real> so we have more accurate time at Send time.
            .add_systems(
                PostUpdate,
                (
                    (send::<P>, clean_prespawned_entity::<P>).in_set(MainSet::SendPackets),
                    sync_update::<P>.in_set(MainSet::Sync),
                ),
            );
    }
}
