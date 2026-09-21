//! We call running an app in 'Host-Server' mode an app that has both the Client and Server plugins, and where one of the client acts as the 'Host'.
//!
//! A Client is considered a host-server if it is:
//! - Connected
//! - is a ClientOf of a Server
//! - the Server is started

#[cfg(feature = "server")]
use alloc::string::String;
use alloc::vec::Vec;

#[cfg(feature = "server")]
use crate::admission::{admit, free_local_alias, reject};
#[cfg(feature = "server")]
use crate::{
    client::{
        Client, Connect, Connected, Connecting, Disconnect, Disconnected, DisconnectedReason,
    },
    client_of::ClientOf,
    server::Started,
};
use bevy_app::{App, Plugin};
use bevy_ecs::prelude::*;
#[cfg(feature = "server")]
use bevy_ecs::{lifecycle::HookContext, world::DeferredWorld};
use bevy_reflect::Reflect;
use bytes::Bytes;
#[cfg(feature = "server")]
use lightyear_core::id::{LocalId, PeerId, RemoteId};
use lightyear_core::tick::Tick;
#[cfg(feature = "server")]
use lightyear_link::prelude::{Endpoint, LinkOf, Server};
#[cfg(feature = "server")]
use tracing::info;

// we want the component to be available even if the server feature is not enabled
/// Marker component inserted on a client that acts as a Host
#[derive(Component, Debug)]
#[cfg_attr(feature = "server", component(on_remove = HostClient::on_remove))]
pub struct HostClient {
    // TODO: put the buffer in a separate component?
    // buffer that will hold the (bytes, channel_kind, tick) for messages serialized by the ServerMultiSender
    pub buffer: Vec<(Bytes, core::any::TypeId, Tick)>,
}

#[cfg(feature = "server")]
impl HostClient {
    fn on_remove(mut world: DeferredWorld, context: HookContext) {
        let Some(endpoint) = world
            .get::<LinkOf>(context.entity)
            .map(|link| link.endpoint)
        else {
            return;
        };
        world.commands().queue(move |world: &mut World| {
            if world
                .get::<HostServer>(endpoint)
                .is_some_and(|host| host.client == context.entity)
            {
                world.entity_mut(endpoint).remove::<HostServer>();
            }
        });
    }
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
    /// Starting the server link can be asynchronous, so the client may request a connection before
    /// the server is [`Started`]. The client remains [`Connecting`] until the server starts.
    #[cfg(feature = "server")]
    fn connect(
        trigger: On<Connect>,
        mut commands: Commands,
        query: Query<&LinkOf, (With<Client>, Without<HostClient>, Without<crate::p2p::P2P>)>,
        server_query: Query<(), With<Server>>,
    ) {
        let Ok(link_of) = query.get(trigger.entity) else {
            return;
        };
        if server_query.get(link_of.endpoint).is_err() {
            return;
        }
        let entity = trigger.entity;
        let endpoint = link_of.endpoint;
        commands.queue(move |world: &mut World| {
            if world.get::<Started>(endpoint).is_none() {
                world.entity_mut(entity).insert(Connecting);
                return;
            }
            if world
                .get::<HostServer>(endpoint)
                .is_some_and(|host| host.client != entity)
            {
                reject(
                    world,
                    entity,
                    String::from("Server already has a host client"),
                );
                return;
            }
            let requested_local = world.get::<LocalId>(entity).map(|id| id.0);
            let requested_remote = world.get::<RemoteId>(entity).map(|id| id.0);
            let id = requested_local
                .or(requested_remote)
                .unwrap_or_else(|| free_local_alias(world, entity, PeerId(u64::MAX)));
            if id == PeerId::SERVER
                || requested_local
                    .zip(requested_remote)
                    .is_some_and(|(local, remote)| local != remote)
            {
                reject(
                    world,
                    entity,
                    String::from("Host identity requests must agree and be nonzero"),
                );
                return;
            }
            // HostClient observers also need the resolved identity for existing-entity backfill.
            // Connected is exposed only after admission registers that identity.
            world
                .entity_mut(endpoint)
                .insert(HostServer { client: entity });
            if admit(
                world,
                entity,
                LocalId(id),
                RemoteId(id),
                (ClientOf, HostClient { buffer: Vec::new() }),
            ) {
                info!(?entity, "Connected host-client");
            } else if world
                .get::<HostServer>(endpoint)
                .is_some_and(|host| host.client == entity)
            {
                world.entity_mut(endpoint).remove::<HostServer>();
            }
        });
    }

    #[cfg(feature = "server")]
    fn disconnect(
        trigger: On<Disconnect>,
        mut commands: Commands,
        query: Query<&LinkOf, (With<Client>, Or<(With<HostClient>, With<Connecting>)>)>,
    ) {
        if query.contains(trigger.entity) {
            info!(entity=?trigger.entity,"Disconnected host-client");
            commands
                .entity(trigger.entity)
                .remove::<HostClient>()
                .insert(Disconnected {
                    reason: DisconnectedReason::UserRequested(None),
                });
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
            && server_query.get(link_of.endpoint).is_ok()
        {
            commands
                .entity(trigger.entity)
                .insert(HostClient { buffer: Vec::new() });
            commands.entity(link_of.endpoint).insert(HostServer {
                client: trigger.entity,
            });
        }
    }

    #[cfg(feature = "server")]
    fn check_if_host_on_server_change(
        trigger: On<Add, (Server, Started)>,
        endpoint_query: Query<&Endpoint, With<Started>>,
        client_query: Query<(Has<Connected>, Has<Connecting>), (With<Client>, Without<HostClient>)>,
        mut commands: Commands,
    ) {
        if let Ok(endpoint) = endpoint_query.get(trigger.entity) {
            for client in endpoint.collection() {
                if let Ok((connected, connecting)) = client_query.get(*client) {
                    if connecting {
                        // The client may have requested a connection before the server link was
                        // ready. `connect` leaves that request pending until `Started` is added, so
                        // retry it now that the server link is ready.
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
    #[test]
    fn host_admission_exposes_role_and_registry_before_connected() {
        let mut app = test_app();
        app.add_observer(
            |trigger: On<Add, (Connected, HostClient)>,
             clients: Query<(
                &Connected,
                &HostClient,
                &ClientOf,
                &LinkOf,
                &LocalId,
                &RemoteId,
            )>,
             servers: Query<&HostServer>,
             metadata: Res<NetworkingMetadata>| {
                let (_, _, _, link, local, remote) = clients.get(trigger.entity).unwrap();
                assert_ne!(local.0, PeerId::SERVER);
                assert_eq!(local.0, remote.0);
                assert_eq!(metadata.peer_map.get(&remote.0), Some(&trigger.entity));
                assert_eq!(servers.get(link.endpoint).unwrap().client, trigger.entity);
            },
        );
        let (server, client) = spawn_host(&mut app);
        app.world_mut()
            .entity_mut(server)
            .insert(crate::admission::NoDefaultConnection);
        app.world_mut().entity_mut(server).insert(Started);
        app.world_mut().trigger(Connect { entity: client });
        app.world_mut().flush();
        assert!(app.world().get::<Connected>(client).is_some());
    }

    use crate::client::ConnectionPlugin;
    use crate::network_topology::NetworkingMetadata;

    fn test_app() -> App {
        let mut app = App::new();
        app.add_plugins((ConnectionPlugin, HostPlugin));
        app
    }

    fn spawn_host(app: &mut App) -> (Entity, Entity) {
        let server = app.world_mut().spawn(Server::default()).id();
        let client = app
            .world_mut()
            .spawn((Client, LinkOf { endpoint: server }))
            .id();
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
            LocalId(PeerId(73)),
            RemoteId(PeerId(73)),
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
                .resource::<NetworkingMetadata>()
                .peer_map
                .get(&PeerId(73)),
            Some(&client)
        );
    }

    #[test]
    fn requested_host_alias_is_preserved_without_replacing_an_existing_host() {
        let mut app = test_app();
        let (server, client) = spawn_host(&mut app);
        app.world_mut()
            .entity_mut(client)
            .insert(LocalId(PeerId(73)));
        app.world_mut().entity_mut(server).insert(Started);
        app.world_mut().trigger(Connect { entity: client });
        app.world_mut().flush();
        assert_eq!(
            app.world().get::<LocalId>(client),
            Some(&LocalId(PeerId(73)))
        );
        assert_eq!(
            app.world().get::<RemoteId>(client),
            Some(&RemoteId(PeerId(73)))
        );
        let other = app
            .world_mut()
            .spawn((Client, LinkOf { endpoint: server }, LocalId(PeerId(74))))
            .id();
        app.world_mut().trigger(Connect { entity: other });
        app.world_mut().flush();
        assert!(app.world().get::<Connected>(other).is_none());
        assert!(app.world().get::<Disconnected>(other).is_some());
        assert_eq!(
            app.world().get::<HostServer>(server).unwrap().client,
            client
        );
        assert_eq!(
            app.world()
                .resource::<NetworkingMetadata>()
                .peer_map
                .get(&PeerId(73)),
            Some(&client)
        );
    }

    #[test]
    fn despawned_host_releases_server_for_replacement() {
        let mut app = test_app();
        let (server, client) = spawn_host(&mut app);
        assert!(crate::server::start_server(app.world_mut(), server));
        app.world_mut().trigger(Connect { entity: client });
        app.world_mut().flush();
        assert!(app.world().get::<HostClient>(client).is_some());

        app.world_mut().despawn(client);
        app.world_mut().flush();
        assert!(app.world().get::<HostServer>(server).is_none());

        let replacement = app
            .world_mut()
            .spawn((Client, LinkOf { endpoint: server }))
            .id();
        app.world_mut().trigger(Connect {
            entity: replacement,
        });
        app.world_mut().flush();
        assert!(app.world().get::<Connected>(replacement).is_some());
        assert!(app.world().get::<HostClient>(replacement).is_some());
        assert_eq!(
            app.world().get::<HostServer>(server).unwrap().client,
            replacement
        );
    }

    #[test]
    fn removing_stale_host_preserves_replacement_owner() {
        let mut app = test_app();
        let (server, client) = spawn_host(&mut app);
        assert!(crate::server::start_server(app.world_mut(), server));
        app.world_mut().trigger(Connect { entity: client });
        app.world_mut().flush();

        let replacement = app
            .world_mut()
            .spawn((Client, LinkOf { endpoint: server }))
            .id();
        app.world_mut().entity_mut(server).insert(HostServer {
            client: replacement,
        });
        app.world_mut().entity_mut(client).remove::<HostClient>();
        app.world_mut().flush();
        assert_eq!(
            app.world().get::<HostServer>(server).unwrap().client,
            replacement
        );
    }
}
