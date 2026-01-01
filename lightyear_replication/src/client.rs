use bevy_app::prelude::*;
use bevy_ecs::prelude::*;
use bevy_replicon::prelude::*;
use lightyear_transport::prelude::Transport;
use lightyear_transport::channel::receivers::ChannelReceive;

/// Adds a client messaging backend made for examples to `bevy_replicon`.
pub struct RepliconClientPlugin;

impl Plugin for RepliconClientPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(PreUpdate, receive_packets.in_set(ClientSystems::ReceivePackets))
        .add_systems(PostUpdate, send_packets.in_set(ClientSystems::SendPackets));
    }
}


/// Receive packets from the server and store them in `ClientMessages`.
///
/// This only works if there is a single `Transport` in the world.
fn receive_packets(
    mut clients: Query<(Entity, &mut Transport)>,
    mut messages: ResMut<ClientMessages>,
) {
    if let Ok((_, mut transport)) = clients.single_mut() {
        transport.receivers.iter_mut().for_each(|(channel_id, receiver)| {
            while let Some((remote_tick, message, _message_id)) = receiver.receiver.read_message() {
                messages.insert_received(*channel_id, message)
            }
        });
    }
}

fn send_packets(
    mut client: Single<&mut Transport>,
    mut messages: ResMut<ClientMessages>,
) -> Result<(), BevyError> {
    for (channel_id, message) in messages.drain_sent() {
        client.send_mut_replicon(channel_id, message)?;
    }
    Ok(())
}
