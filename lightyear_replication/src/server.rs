use bevy_app::prelude::*;
use bevy_ecs::prelude::*;

use bevy_replicon::{prelude::*, shared::backend::connected_client::NetworkId};
use lightyear_connection::client::Connect;
use lightyear_core::id::{RemoteId};
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


// TODO: should we only enable this for entities with ReplicationSender?
//  since we only use replicon for replication messages
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