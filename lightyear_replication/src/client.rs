use bevy_app::prelude::*;
use bevy_ecs::prelude::*;
use bevy_state::prelude::*;

use bevy_replicon::prelude::*;
use bevy_replicon::shared::server_entity_map::ServerEntityMap;
use lightyear_connection::client::{Client, Connected};
use lightyear_connection::host::HostClient;
use lightyear_messages::MessageManager;
use lightyear_transport::channel::receivers::ChannelReceive;
use lightyear_transport::plugin::TransportSystems;
use lightyear_transport::prelude::Transport;

use crate::channels::RepliconChannelMap;
use crate::checkpoint::{
    ReplicationCheckpointMap, extract_server_replicon_tick, unwrap_server_payload,
};
use lightyear_messages::plugin::MessageSystems;
use tracing::{debug, error};

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

        // Despawn all replicated entities when the client disconnects.
        // bevy_replicon's reset only clears the entity map; lightyear must
        // clean up the actual entities.
        app.add_systems(
            OnExit(ClientState::Connected),
            despawn_replicated_on_disconnect.after(ClientSystems::Reset),
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
/// Sets `Connected` only for real remote clients.
///
/// Host-clients intentionally keep Replicon's `ClientState` disconnected so the app behaves like
/// a listen server: replication receive stays disabled and host-local client behavior is emulated
/// directly in the shared world instead.
fn sync_client_state(
    connected: Query<(), (With<Connected>, With<Client>, Without<HostClient>)>,
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
    mut checkpoints: ResMut<ReplicationCheckpointMap>,
    mut transports: Query<&mut Transport, With<Client>>,
) {
    for mut transport in transports.iter_mut() {
        for (idx, &(_, channel_id)) in channel_map.server_channels.iter().enumerate() {
            if let Some(receiver) = transport.receivers.get_mut(&channel_id) {
                while let Some((_, message, _)) = receiver.receiver.read_message() {
                    if idx <= 1 {
                        // Server update / mutation packets are wrapped by Lightyear with the
                        // authoritative simulation tick. We unwrap, record the
                        // RepliconTick -> Tick mapping for prediction/rollback, then hand the
                        // original inner bytes back to Replicon unchanged.
                        match unwrap_server_payload(message).and_then(|(header, inner)| {
                            let replicon_tick = extract_server_replicon_tick(idx, &inner)?;
                            Ok((header, inner, replicon_tick))
                        }) {
                            Ok((header, inner, replicon_tick)) => {
                                checkpoints.record(replicon_tick, header.authoritative_tick);
                                client_messages.insert_received(idx, inner);
                            }
                            Err(error_kind) => {
                                error!(
                                    ?error_kind,
                                    channel_idx = idx,
                                    "dropping malformed wrapped replicon server payload"
                                );
                            }
                        }
                    } else {
                        client_messages.insert_received(idx, message);
                    }
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
    mut transports: Query<&mut Transport, With<Client>>,
) {
    for (channel_idx, message) in client_messages.drain_sent() {
        let (channel_kind, _) = channel_map.client_channels[channel_idx];
        for mut transport in transports.iter_mut() {
            transport
                .send_mut_erased(channel_kind, message.clone(), 1.0)
                .ok();
        }
    }
}

/// Despawn all replicated entities when the client disconnects.
///
/// This matches the old `ReplicationReceivePlugin::handle_disconnection` behavior:
/// all entities that were spawned from replication are despawned on disconnect.
/// In the new replicon flow, predicted and interpolated entities also have `Replicated`
/// since they arrive as replicated components.
fn despawn_replicated_on_disconnect(
    mut commands: Commands,
    replicated: Query<Entity, With<Replicated>>,
) {
    for entity in replicated.iter() {
        debug!("Despawning replicated entity {:?} on disconnect", entity);
        commands.entity(entity).try_despawn();
    }
}

/// Sync replicon's `ServerEntityMap` entries to lightyear's `MessageManager.entity_mapper`.
///
/// This bridges replicon's entity tracking with lightyear's messaging entity map.
fn sync_entity_map(
    entity_map: Res<ServerEntityMap>,
    mut managers: Query<&mut MessageManager, With<Client>>,
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
