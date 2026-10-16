//! Module to verify that the protocols of the client and server match

use bevy_app::{App, Plugin};
use bevy_ecs::prelude::*;
use bevy_replicon::prelude::ProtocolHash;
use lightyear_connection::client::{
    Client, Connected, Disconnect, Disconnected, DisconnectedReason, ProtocolMismatch,
};
use lightyear_connection::client_of::ClientOf;
use lightyear_connection::direction::NetworkDirection;
use lightyear_connection::host::HostClient;
use lightyear_core::id::RemoteId;
use lightyear_messages::prelude::{AppTriggerExt, EventSender, RemoteEvent};
use lightyear_messages::registry::MessageRegistry;
use lightyear_replication::metadata::MetadataChannel;
use lightyear_replication::registry::ComponentRegistry;
use lightyear_transport::prelude::{
    AppChannelExt, ChannelMode, ChannelRegistry, ChannelSettings, ReliableSettings,
};
use serde::{Deserialize, Serialize};
use tracing::{error, trace};

#[derive(Serialize, Deserialize, Debug, PartialEq, Default, Clone, Copy, Event)]
pub struct ProtocolCheck {
    messages: Option<u64>,
    /// Components are registered through Replicon, whose hash covers its replication rules and
    /// receive markers.
    components: Option<ProtocolHash>,
    channels: Option<u64>,
}

pub struct ProtocolCheckPlugin;

impl ProtocolCheckPlugin {
    /// On the server, send a message with the protocol checksum when a client connects
    fn send_verify_protocol(
        trigger: On<Add<Connected>>,
        mut sender: Query<&mut EventSender<ProtocolCheck>, (With<ClientOf>, Without<HostClient>)>,
        messages: Option<ResMut<MessageRegistry>>,
        components: Option<Res<ProtocolHash>>,
        channels: Option<ResMut<ChannelRegistry>>,
    ) {
        let check_message = ProtocolCheck {
            messages: messages.map(|mut m| m.finish()),
            components: components.map(|c| *c),
            channels: channels.map(|mut c| c.finish()),
        };
        if let Ok(mut s) = sender.get_mut(trigger.entity) {
            s.trigger::<MetadataChannel>(check_message);
        }
    }

    /// On the client, disconnect from a server whose protocol differs from ours.
    fn receive_verify_protocol(
        trigger: On<RemoteEvent<ProtocolCheck>>,
        messages: Option<ResMut<MessageRegistry>>,
        components: Option<Res<ProtocolHash>>,
        channels: Option<ResMut<ChannelRegistry>>,
        clients: Query<(Entity, &RemoteId), With<Client>>,
        mut commands: Commands,
    ) {
        let message = trigger.trigger;
        trace!("Received protocol check from server: {message:?}");
        let mismatch = if message.messages != messages.map(|mut m| m.finish()) {
            ProtocolMismatch::Message
        } else if message.components != components.map(|c| *c) {
            ProtocolMismatch::Component
        } else if message.channels != channels.map(|mut c| c.finish()) {
            ProtocolMismatch::Channel
        } else {
            return;
        };
        for (client, remote) in &clients {
            if remote.0 != trigger.from {
                continue;
            }
            error!(?client, "Disconnecting from the server: {mismatch}");
            commands.trigger(Disconnect { entity: client });
            // The transport's disconnect inserts its own reason, which this one replaces:
            // `Add<Disconnected>` observers see the transport's reason, `Insert` ones see this.
            // The order is fixed, as the transport skips an entity already `Disconnected`.
            commands.entity(client).insert(Disconnected {
                reason: DisconnectedReason::ProtocolMismatch(mismatch),
            });
        }
    }
}

impl Plugin for ProtocolCheckPlugin {
    fn build(&self, app: &mut App) {
        // TODO: add these observers only on the server/client
        // app.add_observer(Self::send_verify_protocol);
        // app.add_observer(Self::receive_verify_protocol);

        // try to re-add the Channel in case Replication is not enabled
        app.add_channel::<MetadataChannel>(ChannelSettings {
            mode: ChannelMode::UnorderedReliable(ReliableSettings::default()),
            priority: 10.0,
            ..Default::default()
        })
        .add_direction(NetworkDirection::Bidirectional);

        app.register_event::<ProtocolCheck>()
            .add_direction(NetworkDirection::ServerToClient);
    }

    fn finish(&self, app: &mut App) {
        // Replication can be enabled without any user-defined components. In that case,
        // make sure the registry exists on both sides so the protocol check compares
        // empty registries rather than `Some(...)` vs `None`.
        if !app.world().contains_resource::<ComponentRegistry>() {
            app.world_mut().init_resource::<ComponentRegistry>();
        }

        app.add_observer(Self::send_verify_protocol);
        app.add_observer(Self::receive_verify_protocol);
    }

    // fn finish(&self, app: &mut App) {
    //     // try to re-add the Channel in case Replication is not enabled
    //     app.add_channel::<MetadataChannel>(ChannelSettings {
    //         mode: ChannelMode::UnorderedReliable(ReliableSettings::default()),
    //         send_frequency: Duration::default(),
    //         priority: 10.0,
    //     })
    //     .add_direction(NetworkDirection::Bidirectional);
    //
    //     app.add_trigger::<ProtocolCheck>()
    //         .add_direction(NetworkDirection::ServerToClient);
    // }
}

#[cfg(test)]
mod tests {
    use super::ProtocolCheckPlugin;
    use bevy_app::App;
    use lightyear_replication::registry::ComponentRegistry;

    #[test]
    fn protocol_check_initializes_empty_component_registry() {
        let mut app = App::new();

        app.add_plugins(ProtocolCheckPlugin);
        app.finish();

        assert!(app.world().contains_resource::<ComponentRegistry>());
    }
}
