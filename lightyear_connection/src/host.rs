//! We call running an app in 'Host-Server' mode an app that has both the Client and Server plugins, and where one of the client acts as the 'Host'.
//!
//! A Client is considered a host-server if it is:
//! - Connected
//! - is a ClientOf of a Server
//! - the Server is started

#[cfg(feature = "server")]
use crate::{
    client::{Client, Connect, Connected, Disconnect, Disconnected},
    client_of::ClientOf,
    server::Started,
};
use bevy::prelude::*;
use bytes::Bytes;
#[cfg(feature = "server")]
use lightyear_core::id::{LocalId, PeerId, RemoteId};
#[cfg(feature = "server")]
use lightyear_link::prelude::{LinkOf, Server};

// we want the component to be available even if the server feature is not enabled
/// Marker component inserted on a client that acts as a Host
#[derive(Component, Debug)]
pub struct HostClient {
    // TODO: put the buffer in a separate component?
    // buffer that will hold the (bytes, channel_kind) for messages serialized by the ServerMultiSender
    pub buffer: Vec<(Bytes, core::any::TypeId)>,
}

#[cfg(feature = "server")]
/// Marker component inserted on a server that has a [`HostClient`]
#[derive(Component, Debug, Reflect)]
pub struct HostServer {
    client: Entity,
}

pub struct HostPlugin;

impl HostPlugin {
    // TODO: also add check that the client has LocalIo?

    /// A host-server client gets connected automatically
    #[cfg(feature = "server")]
    fn connect(
        trigger: Trigger<Connect>,
        mut commands: Commands,
        query: Query<&LinkOf, With<Client>>,
        server_query: Query<(), With<Server>>,
    ) {
        if let Ok(link_of) = query.get(trigger.target()) {
            if server_query.get(link_of.server).is_ok() {
                info!("Connected host-client");
                commands.entity(trigger.target()).insert((
                    Connected,
                    // We cannot insert the ids purely from the point of view of the client
                    // so we set both its to Local
                    LocalId(PeerId::Local(0)),
                    RemoteId(PeerId::Local(0)),
                    ClientOf,
                ));
            }
        }
    }

    #[cfg(feature = "server")]
    fn disconnect(
        trigger: Trigger<Disconnect>,
        mut commands: Commands,
        query: Query<&LinkOf, With<Client>>,
        server_query: Query<(), With<Server>>,
    ) {
        if let Ok(link_of) = query.get(trigger.target()) {
            if server_query.get(link_of.server).is_ok() {
                info!("Disconnected host-client");
                commands.entity(trigger.target()).remove::<Connected>();
                commands.entity(trigger.target()).insert(Disconnected {
                    reason: Some("Client trigger".to_string()),
                });
            }
        }
    }

    #[cfg(feature = "server")]
    fn check_if_host_on_client_change(
        // NOTE: we handle Connecting in the trigger because otherwise the client
        //  would never be Connected
        trigger: Trigger<OnAdd, (Client, Connected, LinkOf)>,
        client_query: Query<&LinkOf, (With<Client>, With<Connected>)>,
        server_query: Query<(), (With<Started>, With<Server>)>,
        mut commands: Commands,
    ) {
        if let Ok(link_of) = client_query.get(trigger.target()) {
            if server_query.get(link_of.server).is_ok() {
                commands
                    .entity(trigger.target())
                    .insert(HostClient { buffer: Vec::new() });
                commands.entity(link_of.server).insert(HostServer {
                    client: trigger.target(),
                });
            }
        }
    }

    #[cfg(feature = "server")]
    fn check_if_host_on_server_change(
        trigger: Trigger<OnAdd, (Server, Started)>,
        server_query: Query<&Server, With<Started>>,
        client_query: Query<(), (With<Client>, With<Connected>)>,
        mut commands: Commands,
    ) {
        if let Ok(server) = server_query.get(trigger.target()) {
            for client in server.collection() {
                if client_query.get(*client).is_ok() {
                    commands
                        .entity(*client)
                        .insert(HostClient { buffer: Vec::new() });
                    commands.entity(trigger.target()).insert(HostServer {
                        client: trigger.target(),
                    });
                }
            }
        }
    }
}

impl Plugin for HostPlugin {
    fn build(&self, app: &mut App) {
        #[cfg(feature = "server")]
        app.add_observer(Self::connect);
        #[cfg(feature = "server")]
        app.add_observer(Self::disconnect);
        #[cfg(feature = "server")]
        app.add_observer(Self::check_if_host_on_client_change);
        #[cfg(feature = "server")]
        app.add_observer(Self::check_if_host_on_server_change);
    }
}
