//! We call running an app in 'Host-Server' mode an app that has both the Client and Server plugins, and where one of the client acts as the 'Host'.
//!
//! A Client is considered a host-server if it is:
//! - Connected
//! - is a ClientOf of a Server
//! - the Server is started

#[cfg(feature = "server")]
use alloc::string::ToString;
use alloc::vec::Vec;

#[cfg(feature = "server")]
use crate::{
    client::{Client, Connect, Connected, Connecting, Disconnect, Disconnected},
    client_of::ClientOf,
    server::Started,
};
use bevy_app::{App, Plugin};
use bevy_ecs::prelude::*;
use bevy_reflect::Reflect;
use bytes::Bytes;
#[cfg(feature = "server")]
use lightyear_core::id::{LocalId, PeerId, RemoteId};
use lightyear_core::tick::Tick;
#[cfg(feature = "server")]
use lightyear_link::prelude::{LinkOf, Server};
#[cfg(feature = "server")]
use tracing::info;

// we want the component to be available even if the server feature is not enabled
/// Marker component inserted on a client that acts as a Host
#[derive(Component, Debug)]
pub struct HostClient {
    // TODO: put the buffer in a separate component?
    // buffer that will hold the (bytes, channel_kind, tick) for messages serialized by the ServerMultiSender
    pub buffer: Vec<(Bytes, core::any::TypeId, Tick)>,
}

/// Marker component inserted on a server that has a [`HostClient`]
#[derive(Component, Debug, Reflect)]
pub struct HostServer {
    client: Entity,
}

pub struct HostPlugin;

impl HostPlugin {
    // TODO: also add check that the client has LocalIo?

    /// A host-server client gets connected automatically to the server.
    ///
    /// If the server is not started yet, the client remains [`Connecting`] until the server starts.
    #[cfg(feature = "server")]
    fn connect(
        trigger: On<Connect>,
        mut commands: Commands,
        query: Query<&LinkOf, (With<Client>, Without<HostClient>)>,
        server_query: Query<Has<Started>, With<Server>>,
    ) {
        let Ok(link_of) = query.get(trigger.entity) else {
            return;
        };
        let Ok(server_started) = server_query.get(link_of.server) else {
            return;
        };
        if !server_started {
            commands.entity(trigger.entity).insert(Connecting);
            return;
        }

        info!(entity=?trigger.entity, "Connected host-client");
        commands.entity(trigger.entity).insert((
            Connected,
            // We cannot insert the ids purely from the point of view of the client
            // so we set both its to Local
            LocalId(PeerId::Local(0)),
            RemoteId(PeerId::Local(0)),
            ClientOf,
            // NOTE: it's very important to insert Connected and HostClient at the same time
            //  to avoid race conditions between observers that depend on Connected, and those
            // that depend on HostClient
            HostClient { buffer: Vec::new() },
        ));
        commands.entity(link_of.server).insert(HostServer {
            client: trigger.entity,
        });
    }

    #[cfg(feature = "server")]
    fn disconnect(
        trigger: On<Disconnect>,
        mut commands: Commands,
        query: Query<&LinkOf, (With<Client>, Or<(With<HostClient>, With<Connecting>)>)>,
        server_query: Query<&HostServer>,
    ) {
        if let Ok(link_of) = query.get(trigger.entity) {
            info!(entity=?trigger.entity,"Disconnected host-client");
            commands
                .entity(trigger.entity)
                .remove::<HostClient>()
                .insert(Disconnected {
                    reason: Some("Client trigger".to_string()),
                });
            if server_query
                .get(link_of.server)
                .is_ok_and(|host_server| host_server.client == trigger.entity)
            {
                commands.entity(link_of.server).remove::<HostServer>();
            }
        }
    }

    #[cfg(feature = "server")]
    fn check_if_host_on_client_change(
        // NOTE: we handle Connecting in the trigger because otherwise the client
        //  would never be Connected
        trigger: On<Add, (Client, Connected, LinkOf)>,
        client_query: Query<&LinkOf, (With<Client>, With<Connected>, Without<HostClient>)>,
        server_query: Query<(), (With<Started>, With<Server>)>,
        mut commands: Commands,
    ) {
        if let Ok(link_of) = client_query.get(trigger.entity)
            && server_query.get(link_of.server).is_ok()
        {
            commands
                .entity(trigger.entity)
                .insert(HostClient { buffer: Vec::new() });
            commands.entity(link_of.server).insert(HostServer {
                client: trigger.entity,
            });
        }
    }

    #[cfg(feature = "server")]
    fn check_if_host_on_server_change(
        trigger: On<Add, (Server, Started)>,
        server_query: Query<&Server, With<Started>>,
        client_query: Query<(Has<Connected>, Has<Connecting>), (With<Client>, Without<HostClient>)>,
        mut commands: Commands,
    ) {
        if let Ok(server) = server_query.get(trigger.entity) {
            for client in server.collection() {
                if let Ok((connected, connecting)) = client_query.get(*client) {
                    if connecting {
                        commands.trigger(Connect { entity: *client });
                    } else if connected {
                        commands
                            .entity(*client)
                            .insert(HostClient { buffer: Vec::new() });
                        commands
                            .entity(trigger.entity)
                            .insert(HostServer { client: *client });
                    }
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

#[cfg(all(test, feature = "server"))]
mod tests {
    use super::*;
    use crate::client::{ConnectionPlugin, PeerMetadata};

    fn test_app() -> App {
        let mut app = App::new();
        app.add_plugins((ConnectionPlugin, HostPlugin));
        app
    }

    fn spawn_host(app: &mut App) -> (Entity, Entity) {
        let server = app.world_mut().spawn(Server::default()).id();
        let client = app.world_mut().spawn((Client, LinkOf { server })).id();
        (server, client)
    }

    #[test]
    fn connect_is_retried_when_server_starts() {
        let mut app = test_app();
        let (server, client) = spawn_host(&mut app);

        app.world_mut().trigger(Connect { entity: client });
        app.world_mut().flush();

        assert!(app.world().entity(client).contains::<Connecting>());
        assert!(!app.world().entity(client).contains::<Connected>());

        app.world_mut().entity_mut(server).insert(Started);
        app.world_mut().flush();

        assert!(app.world().entity(client).contains::<Connected>());
        assert!(!app.world().entity(client).contains::<Connecting>());
        assert!(app.world().entity(client).contains::<HostClient>());
        assert_eq!(
            app.world()
                .entity(server)
                .get::<HostServer>()
                .unwrap()
                .client,
            client
        );
    }

    #[test]
    fn disconnect_cancels_pending_connection() {
        let mut app = test_app();
        let (server, client) = spawn_host(&mut app);

        app.world_mut().trigger(Connect { entity: client });
        app.world_mut().flush();
        app.world_mut().trigger(Disconnect { entity: client });
        app.world_mut().flush();

        assert!(app.world().entity(client).contains::<Disconnected>());
        assert!(!app.world().entity(client).contains::<Connecting>());

        app.world_mut().entity_mut(server).insert(Started);
        app.world_mut().flush();

        assert!(!app.world().entity(client).contains::<Connected>());
        assert!(!app.world().entity(client).contains::<HostClient>());
        assert!(!app.world().entity(server).contains::<HostServer>());
    }

    #[test]
    fn connected_client_becomes_host_when_server_starts() {
        let mut app = test_app();
        let (server, client) = spawn_host(&mut app);
        app.world_mut().entity_mut(client).insert((
            LocalId(PeerId::Local(0)),
            RemoteId(PeerId::Local(0)),
            Connected,
        ));
        app.world_mut().flush();

        assert!(!app.world().entity(client).contains::<HostClient>());

        app.world_mut().entity_mut(server).insert(Started);
        app.world_mut().flush();

        assert!(app.world().entity(client).contains::<HostClient>());
        assert_eq!(
            app.world()
                .entity(server)
                .get::<HostServer>()
                .unwrap()
                .client,
            client
        );
        assert_eq!(
            app.world()
                .resource::<PeerMetadata>()
                .mapping
                .get(&PeerId::Local(0)),
            Some(&client)
        );
    }
}
