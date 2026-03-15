use bevy_app::prelude::*;
use bevy_ecs::prelude::*;
use bevy_state::prelude::*;

use bevy_replicon::prelude::*;
use bevy_replicon::shared::server_entity_map::ServerEntityMap;
use lightyear_connection::client::Connected;
use lightyear_messages::MessageManager;
use lightyear_transport::channel::receivers::ChannelReceive;
use lightyear_transport::plugin::TransportSystems;
use lightyear_transport::prelude::Transport;

use lightyear_messages::plugin::MessageSystems;
use tracing::trace;
use crate::channels::RepliconChannelMap;

/// Adds the replicon client-side backend bridge for lightyear.
///
/// Handles:
/// - `ClientState` transitions (Connected when client connects)
/// - Receiving `ClientMessages` (replication data from server) via transport
/// - Sending `ClientMessages` (acks) via transport
/// - Syncing replicon's `ServerEntityMap` to lightyear's `MessageManager` entity mapper
pub struct RepliconClientPlugin;

impl Plugin for RepliconClientPlugin {
    fn build(&self, app: &mut App) {
        // State management
        app.add_systems(
            PreUpdate,
            sync_client_state.before(ClientSystems::ReceivePackets),
        );

        // Packet bridge: replicon <-> lightyear transport
        app.add_systems(
            PreUpdate,
            receive_client_packets.in_set(ClientSystems::ReceivePackets),
        );
        app.add_systems(
            PostUpdate,
            send_client_packets.in_set(ClientSystems::SendPackets),
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
            ClientSystems::ReceivePackets
                .after(TransportSystems::Receive)
                .before(MessageSystems::Receive),
        );
        app.configure_sets(
            PostUpdate,
            ClientSystems::SendPackets.before(TransportSystems::Send),
        );
    }
}

/// Sync replicon's `ClientState` with lightyear lifecycle.
///
/// Sets `Connected` when any entity has `Connected` component (lightyear's connection marker).
fn sync_client_state(
    connected: Query<(), With<Connected>>,
    state: Res<State<ClientState>>,
    mut next_state: ResMut<NextState<ClientState>>,
) {
    if !connected.is_empty() && *state.get() != ClientState::Connected {
        next_state.set(ClientState::Connected);
    }
    if connected.is_empty() && *state.get() != ClientState::Disconnected {
        next_state.set(ClientState::Disconnected);
    }
}

/// Receive packets from transports and populate `ClientMessages` (replication data from server).
///
/// Reads from server_channels (Updates, Mutations) on each transport.
fn receive_client_packets(
    channel_map: Res<RepliconChannelMap>,
    mut client_messages: ResMut<ClientMessages>,
    mut transports: Query<&mut Transport>,
) {
    for mut transport in transports.iter_mut() {
        for (idx, &(_, channel_id)) in channel_map.server_channels.iter().enumerate() {
            if let Some(receiver) = transport.receivers.get_mut(&channel_id) {
                while let Some((_, message, _)) = receiver.receiver.read_message() {
                    client_messages.insert_received(idx, message);
                }
            }
        }
    }
}

/// Send `ClientMessages` (acks) via transport to server.
///
/// Drains `ClientMessages` and sends on client_channels (MutationAcks).
fn send_client_packets(
    channel_map: Res<RepliconChannelMap>,
    mut client_messages: ResMut<ClientMessages>,
    mut transports: Query<&mut Transport>,
) {
    for (channel_idx, message) in client_messages.drain_sent() {
        let (channel_kind, _) = channel_map.client_channels[channel_idx];
        for mut transport in transports.iter_mut() {
            transport.send_mut_erased(channel_kind, message.clone(), 1.0).ok();
        }
    }
}

/// Sync replicon's `ServerEntityMap` entries to lightyear's `MessageManager.entity_mapper`.
///
/// This bridges replicon's entity tracking with lightyear's messaging entity map.
fn sync_entity_map(
    entity_map: Res<ServerEntityMap>,
    mut managers: Query<&mut MessageManager>,
    mut synced_entities: Local<bevy_platform::collections::HashSet<Entity>>,
) {
    if !entity_map.is_changed() {
        return;
    }
    let current: bevy_platform::collections::HashSet<Entity> = entity_map
        .to_client()
        .iter()
        .map(|(server_entity, _)| *server_entity)
        .collect();

    for mut mm in managers.iter_mut() {
        for (server_entity, client_entity) in entity_map.to_client().iter() {
            mm.entity_mapper.insert(*server_entity, *client_entity);
        }
        for removed in synced_entities.difference(&current) {
            mm.entity_mapper.remove_by_remote(*removed);
        }
    }

    synced_entities.clone_from(&current);
}
