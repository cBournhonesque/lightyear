//! We call running an app in 'Host-Server' mode an app that has both the Client and Server plugins, and where one of the client acts as the 'Host'.
//!
//! A client is considered a host-server if it is:
//! - Connected
//! - is a ClientOf of a Server
//! - the Server is started

#[cfg(feature = "server")]
use crate::{
    client::{Client, Connected},
    server::Started,
};
use bevy::prelude::*;
#[cfg(feature = "server")]
use lightyear_link::prelude::{LinkOf, Server};

// we want the component to be available even if the server feature is not enabled
/// Marker component inserted on a client that acts as a Host
#[derive(Component, Debug, Reflect)]
pub struct HostClient;

#[cfg(feature = "server")]
/// Marker component inserted on a server that has a [`HostClient`]
#[derive(Component, Debug, Reflect)]
pub struct HostServer {
    client: Entity,
}

pub struct HostPlugin;

impl HostPlugin {
    // TODO: also add check that the client has LocalIo.

    #[cfg(feature = "server")]
    fn check_if_host_on_client_change(
        trigger: Trigger<OnAdd, (Client, Connected)>,
        client_query: Query<&LinkOf, (With<Client>, With<Connected>)>,
        server_query: Query<(), (With<Started>, With<Server>)>,
        mut commands: Commands,
    ) {
        if let Ok(link_of) = client_query.get(trigger.target()) {
            if server_query.get(link_of.server).is_ok() {
                commands.entity(trigger.target()).insert(HostClient);
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
                    commands.entity(*client).insert(HostClient);
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
        app.add_observer(Self::check_if_host_on_client_change);
        #[cfg(feature = "server")]
        app.add_observer(Self::check_if_host_on_server_change);
    }
}
