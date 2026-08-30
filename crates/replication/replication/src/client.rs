use bevy_app::prelude::*;
use bevy_ecs::prelude::*;
use bevy_state::prelude::*;

#[cfg(any(feature = "prediction", feature = "interpolation"))]
use bevy_replicon::client::server_mutate_ticks::ServerMutateTicks;
use bevy_replicon::prelude::*;
use bevy_replicon::shared::server_entity_map::ServerEntityMap;
use lightyear_connection::client::{Client, Connected};
use lightyear_connection::host::HostClient;
use lightyear_messages::MessageManager;
use lightyear_transport::plugin::TransportSystems;
use lightyear_transport::prelude::Transport;

#[cfg(any(feature = "prediction", feature = "interpolation"))]
use crate::ReplicationSystems;
use crate::channels::RepliconChannelMap;
use crate::checkpoint::ReplicationCheckpointMap;
use crate::prelude::Replicated;
use crate::receive::{Persistent, ReplicationReceiver};
use crate::send::Replicate;
use lightyear_messages::plugin::MessageSystems;
use tracing::debug;
#[cfg(any(feature = "prediction", feature = "interpolation"))]
use tracing::error;

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
        app.add_observer(crate::checkpoint::record_authoritative_tick_userdata);

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
        #[cfg(any(feature = "prediction", feature = "interpolation"))]
        app.add_systems(
            PreUpdate,
            sync_last_confirmed_checkpoint
                .after(ClientSystems::Receive)
                .in_set(ReplicationSystems::Receive),
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

        // bevy_replicon's reset only clears the entity map; lightyear must clean up the actual
        // entities when their receiver disconnects or is removed.
        app.add_observer(on_replication_disconnect);

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

#[cfg(any(feature = "prediction", feature = "interpolation"))]
fn sync_last_confirmed_checkpoint(
    server_mutate_ticks: Res<ServerMutateTicks>,
    mut checkpoints: ResMut<ReplicationCheckpointMap>,
    connected_receivers: Query<
        (),
        (
            With<Connected>,
            With<Client>,
            With<ReplicationReceiver>,
            Without<HostClient>,
        ),
    >,
) {
    // Receiver cleanup clears the checkpoint map before Replicon's state transition resets its
    // mutate ticks. Do not compare those two resources during that short transition window.
    if connected_receivers.is_empty() {
        return;
    }

    let Some(replicon_tick) = server_mutate_ticks.last_confirmed_tick() else {
        return;
    };
    if checkpoints
        .record_last_confirmed_checkpoint(replicon_tick)
        .is_none()
    {
        error!(
            ?replicon_tick,
            "missing authoritative checkpoint mapping for completed mutate tick"
        );
        debug_assert!(
            false,
            "missing authoritative checkpoint mapping for completed mutate tick"
        );
    }
}

/// Sync replicon's `ClientState` with lightyear lifecycle.
///
/// Sets `Connected` only for real remote clients that can receive replication.
///
/// Host-clients intentionally keep Replicon's `ClientState` disconnected so the app behaves like
/// a listen server: replication receive stays disabled and host-local client behavior is emulated
/// directly in the shared world instead.
fn sync_client_state(
    connected: Query<
        (),
        (
            With<Connected>,
            With<Client>,
            With<ReplicationReceiver>,
            Without<HostClient>,
        ),
    >,
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
    mut transports: Query<&mut Transport, (With<Client>, With<ReplicationReceiver>)>,
) {
    for mut transport in transports.iter_mut() {
        for (idx, &(_, channel_id)) in channel_map.server_channels.iter().enumerate() {
            if let Some(receiver) = transport.channel_receive_mut(channel_id) {
                while let Some((_, message, _)) = receiver.read_message() {
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
    mut transports: Query<&mut Transport, (With<Client>, With<ReplicationReceiver>)>,
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

/// Cleans up receiver-side replication state when a receiver disconnects or is removed.
///
/// Watching `Remove` for both [`Connected`] and [`ReplicationReceiver`] handles disconnection,
/// explicit receiver removal, and receiver entity despawn with a single observer. Lifecycle
/// `Remove` observers run before the components disappear, so receiver-local [`Persistent`] can
/// still be read here.
fn on_replication_disconnect(
    trigger: On<Remove, (Connected, ReplicationReceiver)>,
    mut commands: Commands,
    receivers: Query<
        Has<Persistent>,
        (
            With<Connected>,
            With<Client>,
            With<ReplicationReceiver>,
            Without<HostClient>,
        ),
    >,
    replicated: Query<Entity, (With<Replicated>, Without<Replicate>, Without<Persistent>)>,
    mut checkpoints: ResMut<ReplicationCheckpointMap>,
) {
    // The tuple observer runs when either component is removed. Only clean up a connection that
    // had both components immediately before this removal, and do not clean up twice if its
    // receiver entity is later despawned.
    let Ok(receiver_is_persistent) = receivers.get(trigger.entity) else {
        return;
    };

    checkpoints.clear();

    if receiver_is_persistent {
        debug!("Keeping replicated entities because the replication receiver is persistent");
        return;
    }

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

    for mut mm in managers.iter_mut() {
        for (server_entity, client_entity) in entity_map.to_client().iter() {
            mm.entity_mapper.insert(*server_entity, *client_entity);
        }
        for server_entity in synced_entities.iter() {
            if !entity_map.to_client().contains_key(server_entity) {
                mm.entity_mapper.remove_by_remote(*server_entity);
            }
        }
    }

    synced_entities.retain(|server_entity| entity_map.to_client().contains_key(server_entity));
    synced_entities.extend(entity_map.to_client().keys().copied());
}
