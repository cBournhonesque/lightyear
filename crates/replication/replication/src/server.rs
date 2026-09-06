use bevy_app::prelude::*;
use bevy_ecs::prelude::*;
use bevy_state::prelude::*;

use bevy_replicon::prelude::*;
use bevy_replicon::server::visibility::client_visibility::ClientVisibility;
use bevy_replicon::shared::backend::connected_client::NetworkId;
use lightyear_connection::client::Connected;
use lightyear_connection::client_of::ClientOf;
use lightyear_connection::host::HostClient;
use lightyear_connection::server::Started;
use lightyear_core::id::RemoteId;
use lightyear_link::prelude::Link;
use lightyear_transport::packet::fragment_size_for_min_mtu;
use lightyear_transport::plugin::TransportSystems;
use lightyear_transport::prelude::Transport;

use crate::channels::RepliconChannelMap;
use lightyear_messages::plugin::MessageSystems;
use tracing::{error, trace};

/// Adds the replicon server-side backend bridge for lightyear.
///
/// Handles:
/// - `ServerState` transitions when `Started` is added or removed
/// - `ConnectedClient` insertion for replicon visibility
/// - Sending `ServerMessages` (replication) and receiving `ClientMessages` (acks) via transport
pub struct RepliconServerPlugin;

impl Plugin for RepliconServerPlugin {
    fn build(&self, app: &mut App) {
        // When Connected is added to a link entity, add replicon's ConnectedClient + NetworkId
        app.add_observer(on_client_connected);

        // State management
        app.add_observer(on_server_started);
        app.add_observer(on_server_stopped);

        // Packet bridge: replicon <-> lightyear transport
        app.add_systems(
            PreUpdate,
            receive_server_packets.in_set(ServerSystems::ReceivePackets),
        );
        app.add_systems(
            PostUpdate,
            (
                crate::checkpoint::write_authoritative_tick_userdata.before(ServerSystems::Send),
                send_server_packets.in_set(ServerSystems::SendPackets),
            ),
        );

        app.configure_sets(
            PreUpdate,
            ServerSystems::ReceivePackets
                .after(TransportSystems::Receive)
                // Replicon bridge must read its channels before lightyear's MessagePlugin::recv
                // drains ALL transport receivers (including replicon channels)
                .before(MessageSystems::Receive),
        );
        app.configure_sets(
            PostUpdate,
            ServerSystems::SendPackets.before(TransportSystems::Send),
        );
    }
}

/// When `Connected` is added to a remote client link entity, insert replicon's
/// `ConnectedClient` and `NetworkId` so replicon's packet path can target it.
///
/// Host-clients intentionally do not become replicon `ConnectedClient`s because they share the
/// same world as the server and may otherwise collide with a real remote client's `NetworkId`.
/// They only need `ClientVisibility` for lightyear's same-app visibility hooks.
fn on_client_connected(
    _trigger: On<Add, Connected>,
    remotes: Query<
        (Entity, &RemoteId, &Link),
        (Added<Connected>, With<ClientOf>, Without<HostClient>),
    >,
    hosts: Query<Entity, (Added<Connected>, With<HostClient>)>,
    mut commands: Commands,
) {
    for (entity, remote_id, link) in remotes.iter() {
        let min_mtu = link.min_mtu();
        let Some(max_size) = fragment_size_for_min_mtu(min_mtu) else {
            error!(?entity, min_mtu, "link MTU cannot carry fragment packets");
            continue;
        };
        commands.entity(entity).insert((
            ConnectedClient { max_size },
            NetworkId::new(remote_id.to_bits()),
        ));
    }

    for entity in hosts.iter() {
        commands.entity(entity).insert(ClientVisibility::default());
    }
}

/// Set replicon's `ServerState` to `Running` when the server starts.
fn on_server_started(_trigger: On<Add, Started>, mut next_state: ResMut<NextState<ServerState>>) {
    NextState::set_if_neq(&mut next_state, ServerState::Running);
}

/// Set replicon's `ServerState` to `Stopped` when the server stops or is despawned.
///
/// Bevy emits `Remove` when an entity is despawned, so this also handles teardown that bypasses
/// the `Stopped` marker entirely.
fn on_server_stopped(
    _trigger: On<Remove, Started>,
    mut next_state: ResMut<NextState<ServerState>>,
) {
    NextState::set_if_neq(&mut next_state, ServerState::Stopped);
}

/// Receive packets from transports and populate `ServerMessages` (ack data from peers).
///
/// Reads from client_channels (MutationAcks) on each transport and puts into `ServerMessages`.
fn receive_server_packets(
    channel_map: Res<RepliconChannelMap>,
    mut server_messages: ResMut<ServerMessages>,
    mut transports: Query<(Entity, &mut Transport), With<ClientOf>>,
) {
    for (entity, mut transport) in transports.iter_mut() {
        for (idx, &(_, channel_id)) in channel_map.client_channels.iter().enumerate() {
            if let Some(receiver) = transport.channel_receive_mut(channel_id) {
                while let Some((_, message, _)) = receiver.read_message() {
                    server_messages.insert_received(entity, idx, message);
                }
            }
        }
    }
}

/// Send `ServerMessages` (replication data) via transport to peers.
///
/// Drains `ServerMessages` and stages bytes on server_channels (Updates, Mutations)
/// through the shared-access [`Transport`] queue, so this system only needs a
/// shared borrow and can run in parallel with other producers.
fn send_server_packets(
    channel_map: Res<RepliconChannelMap>,
    mut server_messages: ResMut<ServerMessages>,
    transports: Query<&Transport, With<ClientOf>>,
) {
    for (client, channel_idx, message) in server_messages.drain_sent() {
        let (channel_kind, _) = channel_map.server_channels[channel_idx];
        trace!(
            "send_server_packets: sending {} bytes on channel_idx={} to {:?}",
            message.len(),
            channel_idx,
            client
        );
        if let Ok(transport) = transports.get(client) {
            transport.send_erased(channel_kind, message, 1.0).ok();
        } else {
            trace!("send_server_packets: no transport for client {:?}", client);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{on_client_connected, on_server_started, on_server_stopped};
    use bevy_app::App;
    use bevy_replicon::prelude::ServerState;
    use bevy_replicon::shared::backend::connected_client::{ConnectedClient, NetworkIdMap};
    use bevy_state::app::{AppExtStates, StatesPlugin};
    use bevy_state::state::State;
    use lightyear_connection::client::Connected;
    use lightyear_connection::client_of::ClientOf;
    use lightyear_connection::network_topology::NetworkingMetadata;
    use lightyear_connection::server::{Started, Stopped};
    use lightyear_core::id::{PeerId, RemoteId};
    use lightyear_link::prelude::{Link, LinkMtu, Server};
    use lightyear_transport::packet::fragment_size_for_min_mtu;
    use test_log::test;

    #[test]
    fn connected_client_max_size_uses_link_minimum_mtu() {
        let mut app = App::new();
        app.add_observer(on_client_connected);
        app.init_resource::<NetworkIdMap>();

        let min_mtu = 256;
        let entity = app
            .world_mut()
            .spawn((
                RemoteId(PeerId::Netcode(1)),
                ClientOf,
                Link::default().with_mtu(LinkMtu::new(min_mtu)),
                Connected,
            ))
            .id();
        app.update();

        assert_eq!(
            app.world().get::<ConnectedClient>(entity).unwrap().max_size,
            fragment_size_for_min_mtu(min_mtu).unwrap()
        );
    }

    fn app_with_server_state(state: ServerState) -> App {
        let mut app = App::new();
        app.add_plugins(StatesPlugin)
            .init_resource::<NetworkingMetadata>()
            .init_state::<ServerState>()
            .add_observer(on_server_started)
            .add_observer(on_server_stopped)
            .insert_state(state);
        app
    }

    #[test]
    fn started_lifecycle_transitions_server_state() {
        let mut app = app_with_server_state(ServerState::Stopped);
        let server = app.world_mut().spawn((Server::default(), Started)).id();

        app.update();

        assert_eq!(
            *app.world().resource::<State<ServerState>>().get(),
            ServerState::Running
        );

        app.world_mut().entity_mut(server).insert(Stopped);

        app.update();

        assert_eq!(
            *app.world().resource::<State<ServerState>>().get(),
            ServerState::Stopped
        );
    }

    #[test]
    fn despawned_started_entity_transitions_server_state_to_stopped() {
        let mut app = app_with_server_state(ServerState::Stopped);
        let server = app.world_mut().spawn((Server::default(), Started)).id();

        app.update();
        assert_eq!(
            *app.world().resource::<State<ServerState>>().get(),
            ServerState::Running
        );

        app.world_mut().despawn(server);

        app.update();

        assert_eq!(
            *app.world().resource::<State<ServerState>>().get(),
            ServerState::Stopped
        );
    }

    #[test]
    fn started_and_despawned_in_same_flush_remains_stopped() {
        let mut app = app_with_server_state(ServerState::Stopped);
        let server = app.world_mut().spawn(Server::default()).id();
        app.world_mut()
            .commands()
            .entity(server)
            .insert(Started)
            .despawn();
        app.world_mut().flush();

        app.update();

        assert_eq!(
            *app.world().resource::<State<ServerState>>().get(),
            ServerState::Stopped
        );
    }
}
