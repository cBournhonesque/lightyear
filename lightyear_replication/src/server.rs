use bevy_app::prelude::*;
use bevy_ecs::prelude::*;
use bevy_state::prelude::*;

use bevy_replicon::prelude::*;
use bevy_replicon::shared::backend::connected_client::NetworkId;
use bevy_replicon::shared::server_entity_map::ServerEntityMap;
use lightyear_connection::client::Connected;
use lightyear_connection::server::{Started, Stopped};
use lightyear_core::id::RemoteId;
use lightyear_messages::MessageManager;
use lightyear_transport::channel::receivers::ChannelReceive;
use lightyear_transport::plugin::TransportSystems;
use lightyear_transport::prelude::Transport;

use lightyear_messages::plugin::MessageSystems;
use tracing::{debug, trace};
use crate::channels::RepliconChannelMap;

/// Adds the replicon server-side backend bridge for lightyear.
///
/// Handles:
/// - `ServerState` transitions (Running when server starts or client connects)
/// - `ConnectedClient` insertion for replicon visibility
/// - Sending `ServerMessages` (replication) and receiving `ClientMessages` (acks) via transport
/// - Syncing replicon's entity map to lightyear's `MessageManager` entity mapper
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
            send_server_packets.in_set(ServerSystems::SendPackets),
        );

        // Entity map bridge: replicon's ServerEntityMap -> lightyear's MessageManager entity_mapper
        app.add_systems(
            PreUpdate,
            sync_entity_map
                .after(ClientSystems::Receive)
                .after(ServerSystems::Receive),
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

/// When `Connected` is added to a link entity, insert replicon's
/// `ConnectedClient` and `NetworkId` so replicon's visibility system sees it.
///
/// This fires on both CLIENT and SERVER apps:
/// - SERVER: when a remote client connects (client_of entity)
/// - CLIENT: when the client connects to the server (client entity)
fn on_client_connected(
    _trigger: On<Add, Connected>,
    query: Query<(Entity, &RemoteId), Added<Connected>>,
    mut commands: Commands,
) {
    for (entity, remote_id) in query.iter() {
        commands.entity(entity).insert((
            ConnectedClient {
                max_size: lightyear_transport::packet::packet_builder::MAX_PACKET_SIZE,
            },
            NetworkId::new(remote_id.to_bits()),
        ));
    }
}

/// Sync replicon's `ServerState` with lightyear lifecycle.
///
/// Sets `Running` when `Started` is present (server app).
///
/// For CLIENT → SERVER replication (`Replicate::to_server()`), ServerState is set to Running
/// from the Replicate on_insert hook instead, so the CLIENT app's replicon server only runs
/// when there are entities to replicate. This prevents the CLIENT from sending empty mutations
/// (from `track_mutate_messages`) that would confuse the SERVER's replicon client in multi-client setups.
fn sync_server_state(
    started: Query<(), With<Started>>,
    stopped: Query<(), With<Stopped>>,
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
    mut transports: Query<(Entity, &mut Transport)>,
) {
    for (entity, mut transport) in transports.iter_mut() {
        for (idx, &(_, channel_id)) in channel_map.client_channels.iter().enumerate() {
            if let Some(receiver) = transport.receivers.get_mut(&channel_id) {
                while let Some((_, message, _)) = receiver.receiver.read_message() {
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
    mut transports: Query<&mut Transport>,
) {
    for (client, channel_idx, message) in server_messages.drain_sent() {
        let (channel_kind, _) = channel_map.server_channels[channel_idx];
        trace!("send_server_packets: sending {} bytes on channel_idx={} to {:?}", message.len(), channel_idx, client);
        if let Ok(mut transport) = transports.get_mut(client) {
            transport.send_mut_erased(channel_kind, message, 1.0).ok();
        } else {
            trace!("send_server_packets: no transport for client {:?}", client);
        }
    }
}

/// Sync replicon's `ServerEntityMap` entries to lightyear's `MessageManager.entity_mapper`.
///
/// This bridges replicon's entity tracking with lightyear's messaging entity map.
/// Handles both additions and removals: entities that are in `ServerEntityMap` are added
/// to the entity_mapper, and entities that were previously synced but are no longer in
/// `ServerEntityMap` are removed.
fn sync_entity_map(
    entity_map: Res<ServerEntityMap>,
    mut managers: Query<&mut MessageManager>,
    mut synced_entities: Local<bevy_platform::collections::HashSet<Entity>>,
) {
    if !entity_map.is_changed() {
        return;
    }
    // Collect current replicon entities
    let current: bevy_platform::collections::HashSet<Entity> = entity_map
        .to_client()
        .iter()
        .map(|(server_entity, _)| *server_entity)
        .collect();

    // In replicon: server_entity = remote entity, client_entity = local entity
    // In lightyear: remote_entity = remote, local_entity = local
    // So we map: replicon server_entity -> lightyear remote, replicon client_entity -> lightyear local
    for mut mm in managers.iter_mut() {
        // Add new entries
        for (server_entity, client_entity) in entity_map.to_client().iter() {
            mm.entity_mapper.insert(*server_entity, *client_entity);
        }
        // Remove entries that are no longer in ServerEntityMap
        for removed in synced_entities.difference(&current) {
            mm.entity_mapper.remove_by_remote(*removed);
        }
    }

    synced_entities.clone_from(&current);
}
