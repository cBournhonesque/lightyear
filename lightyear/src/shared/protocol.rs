use std::time::Duration;

use naia_socket_shared::{LinkConditionerConfig, SocketConfig};
use crate::shared::{Channels, Components, Messages};
use crate::shared::messages::channel_config::ChannelSettings;

use super::{Channel, ChannelDirection, ChannelMode, CompressionConfig, Message, Replicate};

#[derive(Clone)]
pub struct Protocol {
    /// Used to configure the underlying socket
    pub socket: SocketConfig,
    /// The duration between each tick
    pub tick_interval: Option<Duration>,
    /// Configuration used to control compression parameters
    pub compression: Option<CompressionConfig>,

    /// List of channels supported by the Protocol
    pub channels: Channels,
    /// List of channels supported by the Protocol
    pub components: Components,
    /// List of channels supported by the Protocol
    pub messages: Messages,
    // TODO: add to messages the list of default messages (entity spawned, etc.)
    // TODO: add to channels the list of default channels
}

impl Protocol {
    pub fn builder() -> ProtocolBuilder {
        ProtocolBuilder {
            link_conditioner_config: None,
            rtc_endpoint_path: None,
            tick_interval: None,
            compression: None,
        }
    }
}

pub struct ProtocolBuilder {
    link_conditioner_config: Option<LinkConditionerConfig>,
    rtc_endpoint_path: Option<String>,
    tick_interval: Option<Duration>,
    compression: Option<CompressionConfig>,
}

impl ProtocolBuilder {
    pub fn add_plugin<P: Plugin>(&mut self, plugin: P) -> &mut Self {
        plugin.build(self);
        self
    }

    pub fn link_condition(&mut self, config: LinkConditionerConfig) -> &mut Self {
        self.link_conditioner_config = Some(config);
        self
    }

    pub fn rtc_endpoint(&mut self, path: String) -> &mut Self {
        self.rtc_endpoint_path = Some(path);
        self
    }

    pub fn tick_interval(&mut self, duration: Duration) -> &mut Self {
        self.tick_interval = Some(duration);
        self
    }

    pub fn compression(&mut self, config: CompressionConfig) -> &mut Self {
        self.compression = Some(config);
        self
    }

    pub fn add_channel<C: Channel>(
        &mut self,
        direction: ChannelDirection,
        mode: ChannelMode,
    ) -> &mut Self {
        Channels::add_channel::<C>(ChannelSettings::new(mode, direction));
    }

    pub fn add_message<M: Message>(&mut self) -> &mut Self {
        todo!()
    }

    pub fn add_component<C: Replicate>(&mut self) -> &mut Self {
        todo!()
    }

    pub fn build(&mut self) -> Protocol {
        let socket = SocketConfig::new(
            self.link_conditioner_config.take(),
            self.rtc_endpoint_path.take(),
        );
        Protocol {
            socket,
            tick_interval: self.tick_interval.take(),
            compression: self.compression.take(),
        }
    }
}

//Plugin
pub trait Plugin {
    fn build(&self, protocol: &mut ProtocolBuilder);
}
