use bevy_app::prelude::*;
use bevy_ecs::prelude::*;
use bevy_state::prelude::*;

use bevy_replicon::prelude::*;
use bevy_replicon::server::visibility::client_visibility::ClientVisibility;
use bevy_replicon::shared::backend::connected_client::NetworkId;
use lightyear_connection::client::Connected;
use lightyear_connection::client_of::ClientOf;
use lightyear_connection::host::HostClient;
use lightyear_connection::server::{Started, Stopped};
use lightyear_core::id::RemoteId;
use lightyear_link::prelude::{Link, Server};
use lightyear_transport::packet::fragment_size_for_min_mtu;
use lightyear_transport::plugin::TransportSystems;
use lightyear_transport::prelude::Transport;

use crate::channels::RepliconChannelMap;
use lightyear_messages::plugin::MessageSystems;
use tracing::{error, trace};

/// Adds the replicon server-side backend bridge for lightyear.
///
/// Handles:
/// - `ServerState` transitions (Running when server starts or client connects)
/// - `ConnectedClient` insertion for replicon visibility
/// - Sending `ServerMessages` (replication) and receiving `ClientMessages` (acks) via transport
pub struct RepliconServerPlugin;

impl Plugin for RepliconServerPlugin {
    fn build(&self, app: &mut App) {
        // When Connected is added to a link entity, add replicon's ConnectedClient + NetworkId
        app.add_observer(on_client_connected);

        // State management
        app.add_systems(
            PreUpdate,
            sync_server_state.before(ServerSystems::ReceivePackets),
        );

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

/// Sync replicon's `ServerState` with lightyear lifecycle.
///
/// Sets `Running` when `Started` is present (server app).
fn sync_server_state(
    started: Query<(), (With<Server>, With<Started>)>,
    stopped: Query<(), (With<Server>, With<Stopped>)>,
    state: Res<State<ServerState>>,
    mut next_state: ResMut<NextState<ServerState>>,
) {
    if !started.is_empty() && *state.get() != ServerState::Running {
        next_state.set(ServerState::Running);
    }
    if started.is_empty() && !stopped.is_empty() && *state.get() != ServerState::Stopped {
        next_state.set(ServerState::Stopped);
    }
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
/// Drains `ServerMessages` and sends on server_channels (Updates, Mutations).
fn send_server_packets(
    channel_map: Res<RepliconChannelMap>,
    mut server_messages: ResMut<ServerMessages>,
    mut transports: Query<&mut Transport, With<ClientOf>>,
) {
    for (client, channel_idx, message) in server_messages.drain_sent() {
        let (channel_kind, _) = channel_map.server_channels[channel_idx];
        trace!(
            "send_server_packets: sending {} bytes on channel_idx={} to {:?}",
            message.len(),
            channel_idx,
            client
        );
        if let Ok(mut transport) = transports.get_mut(client) {
            transport.send_mut_erased(channel_kind, message, 1.0).ok();
        } else {
            trace!("send_server_packets: no transport for client {:?}", client);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{on_client_connected, sync_server_state};
    use bevy_app::{App, Update};
    use bevy_replicon::prelude::ServerState;
    use bevy_replicon::shared::backend::connected_client::{ConnectedClient, NetworkIdMap};
    use bevy_state::app::{AppExtStates, StatesPlugin};
    use bevy_state::state::State;
    use lightyear_connection::client::{Connected, PeerMetadata};
    use lightyear_connection::client_of::ClientOf;
    use lightyear_connection::server::Stopped;
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

    #[test]
    fn non_server_stopped_marker_does_not_stop_local_sender() {
        let mut app = App::new();
        app.add_plugins(StatesPlugin)
            .init_resource::<PeerMetadata>()
            .init_state::<ServerState>()
            .add_systems(Update, sync_server_state)
            .insert_state(ServerState::Running);

        app.world_mut().spawn(Stopped);

        app.update();
        app.update();

        assert_eq!(
            *app.world().resource::<State<ServerState>>().get(),
            ServerState::Running
        );
    }

    #[test]
    fn stopped_server_entity_transitions_state_to_stopped() {
        let mut app = App::new();
        app.add_plugins(StatesPlugin)
            .init_resource::<PeerMetadata>()
            .init_state::<ServerState>()
            .add_systems(Update, sync_server_state)
            .insert_state(ServerState::Running);

        app.world_mut().spawn((Server::default(), Stopped));

        app.update();
        app.update();

        assert_eq!(
            *app.world().resource::<State<ServerState>>().get(),
            ServerState::Stopped
        );
    }
}
