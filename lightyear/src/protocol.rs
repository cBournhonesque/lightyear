//! Module to verify that the protocols of the client and server match

use bevy::prelude::*;
use bevy::reflect::erased_serde::__private::serde::{Deserialize, Serialize};
use core::time::Duration;
use lightyear_connection::client::Connected;
use lightyear_connection::client_of::ClientOf;
use lightyear_connection::direction::NetworkDirection;
use lightyear_connection::host::HostClient;
use lightyear_messages::prelude::{AppTriggerExt, RemoteTrigger, TriggerSender};
use lightyear_messages::registry::MessageRegistry;
use lightyear_replication::message::MetadataChannel;
use lightyear_replication::registry::registry::ComponentRegistry;
use lightyear_transport::prelude::{
    AppChannelExt, ChannelMode, ChannelRegistry, ChannelSettings, ReliableSettings,
};

#[derive(Serialize, Deserialize, Debug, PartialEq, Default, Clone, Copy, Event)]
pub struct ProtocolCheck {
    messages: Option<u64>,
    components: Option<u64>,
    channels: Option<u64>,
}

pub struct ProtocolCheckPlugin;

#[derive(thiserror::Error, Debug)]
pub enum ProtocolCheckError {
    #[error("the message protocol doesn't match")]
    Message,
    #[error("the component protocol doesn't match")]
    Component,
    #[error("the channel protocol doesn't match")]
    Channel,
}

impl ProtocolCheckPlugin {
    /// On the server, send a message with the protocol checksum when a client connects
    fn send_verify_protocol(
        trigger: Trigger<OnAdd, Connected>,
        mut sender: Query<&mut TriggerSender<ProtocolCheck>, (With<ClientOf>, Without<HostClient>)>,
        messages: Option<ResMut<MessageRegistry>>,
        components: Option<ResMut<ComponentRegistry>>,
        channels: Option<ResMut<ChannelRegistry>>,
    ) {
        let check_message = ProtocolCheck {
            messages: messages.map(|mut m| m.finish()),
            components: components.map(|mut c| c.finish()),
            channels: channels.map(|mut c| c.finish()),
        };
        if let Ok(mut s) = sender.get_mut(trigger.target()) {
            s.trigger::<MetadataChannel>(check_message);
        }
    }

    fn receive_verify_protocol(
        trigger: Trigger<RemoteTrigger<ProtocolCheck>>,
        messages: Option<ResMut<MessageRegistry>>,
        components: Option<ResMut<ComponentRegistry>>,
        channels: Option<ResMut<ChannelRegistry>>,
    ) -> Result {
        let message = trigger.trigger;
        trace!("Received protocol check from server: {message:?}");
        if message.messages != messages.map(|mut m| m.finish()) {
            return Err(BevyError::from(ProtocolCheckError::Message));
        }
        if message.components != components.map(|mut c| c.finish()) {
            return Err(BevyError::from(ProtocolCheckError::Component));
        }
        if message.channels != channels.map(|mut c| c.finish()) {
            return Err(BevyError::from(ProtocolCheckError::Channel));
        }
        Ok(())
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
            send_frequency: Duration::default(),
            priority: 10.0,
        })
        .add_direction(NetworkDirection::Bidirectional);

        app.add_trigger::<ProtocolCheck>()
            .add_direction(NetworkDirection::ServerToClient);
    }

    fn finish(&self, app: &mut App) {
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
