//! Defines the client bevy plugin
use std::ops::DerefMut;
use std::sync::Mutex;

use crate::_reexport::ShouldBeInterpolated;
use crate::client::connection::{replication_clean, ConnectionManager};
use crate::client::diagnostics::ClientDiagnosticsPlugin;
use bevy::prelude::*;
use bevy::tasks::IoTaskPool;
use bevy::time::common_conditions::on_timer;
use bevy::transform::TransformSystem;
use bevy::utils::Duration;

use crate::client::events::{ConnectEvent, DisconnectEvent, EntityDespawnEvent, EntitySpawnEvent};
use crate::client::input::InputPlugin;
use crate::client::interpolation::plugin::InterpolationPlugin;
use crate::client::prediction::plugin::{is_connected, is_in_rollback, PredictionPlugin};
use crate::client::prediction::Rollback;
use crate::client::resource::{Authentication, Client};
use crate::client::sync::client_is_synced;
use crate::client::systems::{receive, send, sync_update};
use crate::connection::netcode::CONNECT_TOKEN_BYTES;
use crate::prelude::{ReplicationSet, ShouldBePredicted, TimeManager};
use crate::protocol::component::ComponentProtocol;
use crate::protocol::message::MessageProtocol;
use crate::protocol::Protocol;
use crate::shared::events::ConnectionEvents;
use crate::shared::plugin::SharedPlugin;
use crate::shared::replication::systems::add_replication_send_systems;
use crate::shared::sets::{FixedUpdateSet, MainSet};
use crate::shared::time_manager::{is_ready_to_send, TimePlugin};
use crate::transport::io::Io;
use crate::transport::{PacketReceiver, PacketSender};

use super::config::ClientConfig;

pub struct PluginConfig<P: Protocol> {
    client_config: ClientConfig,
    io: Io,
    protocol: P,
}

impl<P: Protocol> PluginConfig<P> {
    pub fn new(client_config: ClientConfig, io: Io, protocol: P) -> Self {
        PluginConfig {
            client_config,
            io,
            protocol,
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

// fn init_netcode(
//     world: &mut World,
//     // config: Res<ClientConfig>,
//     // auth: Res<Authentication>,
//     // mut io: ResMut<Io>,
//     // task_pool: Res<IoTaskPool>,
// ) {
//     world.resource_scope(|world: &mut World, config: Mut<ClientConfig>| {
//         world.resource_scope(|world: &mut World, auth: Mut<Authentication>| {
//             world.resource_scope(|world: &mut World, mut io: Mut<Io>| {
//                 // TODO: remove clone
//                 let token_bytes = match auth.clone() {
//                     // we want to request the token directly from the server using the secure io
//                     // TODO: check that io is secure
//                     Authentication::RequestConnectToken { server_addr } => {
//                         let request_token = [u8::MAX].as_slice();
//                         let mut connect_token_bytes = [0; CONNECT_TOKEN_BYTES];
//                         let mut first = false;
//                         let mut second = false;
//                         loop {
//                             // sending token request
//                             let _ = io.send(request_token, &server_addr).map_err(|e| {
//                                 error!("could not send request for connect token: {:?}", e)
//                             });
//                             // receive
//                             info!("waiting for connect token response");
//                             if let Ok(Some((data, addr))) = io.recv() {
//                                 info!("received data from server {:?}", data.len());
//                                 if addr == server_addr && data.len() == 1000 {
//                                     // TODO: this is so bad it makes me want to cry
//                                     connect_token_bytes[..1000].copy_from_slice(data);
//                                     first = true;
//                                 }
//                                 if addr == server_addr && data.len() == 1048 {
//                                     connect_token_bytes[1000..].copy_from_slice(data);
//                                     second = true;
//                                 }
//                                 if first && second {
//                                     info!("Received connect token from server");
//                                     break;
//                                 }
//                             }
//                         }
//                         connect_token_bytes
//                     }
//                     _ => {
//                         let token = auth
//                             .clone()
//                             .get_token(config.netcode.client_timeout_secs)
//                             .expect("could not generate token");
//                         token.try_into_bytes().unwrap()
//                     }
//                 };
//                 let netcode = crate::connection::netcode::Client::with_config(
//                     &token_bytes,
//                     config.netcode.build(),
//                 )
//                 .unwrap();
//                 world.insert_resource(netcode);
//             });
//         });
//     });
// }

// TODO: override `ready` and `finish` to make sure that the transport/backend is connected
//  before the plugin is ready
impl<P: Protocol> Plugin for ClientPlugin<P> {
    fn build(&self, app: &mut App) {
        let config = self.config.lock().unwrap().deref_mut().take().unwrap();

        let netclient = config.client_config.net.clone().get_client(config.io);
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
            // .insert_resource(config.auth.clone())
            .insert_resource(config.client_config.clone())
            .insert_resource(netclient)
            .insert_resource(ConnectionManager::<P>::new(
                config.protocol.channel_registry(),
                config.client_config.packet,
                config.client_config.sync,
                config.client_config.ping,
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
                    // the client hash component is not replicated to the server, so there's no ordering constraint
                    ReplicationSet::SetPreSpawnedHash.in_set(ReplicationSet::All),
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
                    // ReplicationSet::All runs once per frame, so we cannot put it in the `Send` set
                    // which runs every send_interval
                    (ReplicationSet::All, MainSet::SendPackets).chain(),
                    // only replicate entities once client is synced
                    // NOTE: we need is_synced, and not connected. Otherwise the ticks associated with the messages might be incorrect
                    //  and the message might ignored by the server
                    //  But then pre-predicted entities that are spawned right away will not be replicated?
                    ReplicationSet::All.run_if(client_is_synced::<P>),
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
            // .add_systems(Startup, init_netcode)
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
