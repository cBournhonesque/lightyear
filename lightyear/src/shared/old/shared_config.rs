use std::{default::Default, time::Duration};

use naia_socket_shared::SocketConfig;

use crate::{
    connection::compression_config::CompressionConfig,
    messages::channel_config::{ChannelConfig, ChannelIndex, DefaultChannels},
    Channel,
};

/// Contains Config properties which will be shared by Server and Client
#[derive(Clone)]
pub struct SharedConfig<C: ChannelIndex> {
    /// Used to configure the underlying socket
    pub socket: SocketConfig,
    /// Config for Message channels
    pub channel: ChannelConfig<C>,
    /// The duration between each tick
    pub tick_interval: Option<Duration>,
    /// Configuration used to control compression parameters
    pub compression: Option<CompressionConfig>,
}

impl<C: ChannelIndex> SharedConfig<C> {
    /// Creates a new SharedConfig
    pub fn new(
        socket: SocketConfig,
        channel: &[Channel<C>],
        tick_interval: Option<Duration>,
        compression: Option<CompressionConfig>,
    ) -> Self {
        let channel_config = ChannelConfig::new(channel);
        Self {
            socket,
            channel: channel_config,
            tick_interval,
            compression,
        }
    }
}

impl SharedConfig<DefaultChannels> {
    /// Creates a new with Default parameters
    pub fn default() -> Self {
        Self::new(
            SocketConfig::default(),
            ChannelConfig::<DefaultChannels>::default(),
            Some(Duration::from_millis(50)),
            None,
        )
    }
}
