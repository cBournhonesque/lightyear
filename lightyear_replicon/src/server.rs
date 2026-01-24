use bevy_app::prelude::*;
use bevy_ecs::prelude::*;
use bevy_time::prelude::Timer;

use bevy_replicon::{prelude::*, shared::backend::connected_client::NetworkId};
use bevy_replicon::server::server_tick::ServerTick;
use bevy_time::Time;
use lightyear_connection::client::Connect;
use lightyear_core::id::{RemoteId};
use lightyear_core::prelude::LocalTimeline;
use lightyear_transport::packet::packet_builder::MAX_PACKET_SIZE;
use lightyear_transport::channel::receivers::ChannelReceive;
use lightyear_transport::prelude::Transport;


/// Adds a server messaging backend made for examples to `bevy_replicon`.
pub struct RepliconServerPlugin;

impl Plugin for RepliconServerPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_connect);
        app.add_systems(PreUpdate, receive_packets.in_set(ServerSystems::ReceivePackets));
        app.add_systems(PostUpdate, send_packets.in_set(ServerSystems::SendPackets));
        app.add_systems(PostUpdate, update_replication_tick.in_set(ServerSystems::IncrementTick));
    }
}

/// Add the replication-components to the Link entity
// TODO: maybe add ConnectedClient if it has a ReplicationSender marker?
fn on_connect(connect: On<Connect>, query: Query<&RemoteId>, mut commands: Commands) {
    if let Ok(remote_id) = query.get(connect.entity) {
        commands.entity(connect.entity).insert((
            ConnectedClient { max_size: MAX_PACKET_SIZE },
            // note: the u64 id could conflict with one from a different type! For example NetcodeId vs SteamId.
            NetworkId::new(remote_id.to_bits()),
        ));
    }
}

fn receive_packets(
    mut messages: ResMut<ServerMessages>,
    mut clients: Query<(Entity, &mut Transport)>,
) {
    clients.iter_mut().for_each(|(client, mut transport)| {
        transport.receivers.iter_mut().for_each(|(channel_id, receiver)| {
            while let Some((_, message, message_id)) = receiver.receiver.read_message() {
                messages.insert_received(client, *channel_id, message)
            }
        })
    });
}

struct ReplicationMetadata {
    timer: Timer,
}


/// Replication is triggered in Replicon every time the `ServerTick` is incremented, which happens every
/// time the `Timer` in `ReplicationMetadata` finishes.
fn update_replication_tick(
    time: Res<Time>,
    timeline: Res<LocalTimeline>,
    mut replication_metadata: ResMut<ReplicationMetadata>,
    mut replication_tick: ResMut<ServerTick>,
) {
    replication_metadata.timer.tick(time.delta());
    if replication_metadata.timer.just_finished() {
        let current_tick = replication_tick.get();
        let new_tick = timeline.tick();
        replication_tick.increment_by(new_tick - current_tick);
    }
}


fn send_packets(
    // mut disconnects: MessageReader<DisconnectRequest>,
    mut messages: ResMut<ServerMessages>,
    mut clients: Query<&mut Transport>,
) -> Result<(), BevyError> {
    for (client, channel_id, message) in messages.drain_sent() {
        let mut transport = clients
            .get_mut(client)
            .expect("all connected clients should have streams");
        transport.send_mut_replicon(channel_id, message)?;
    }

    // for disconnect in disconnects.read() {
    //     commands.entity(disconnect.client).despawn();
    // }
    Ok(())
}