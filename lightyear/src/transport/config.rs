use bevy::prelude::Reflect;

use crate::transport::middleware::{
    compression::CompressionConfig, conditioner::LinkConditionerConfig,
};

#[derive(Clone, Debug, Default, Reflect)]
#[reflect(from_reflect = false)]
pub struct SharedIoConfig<T> {
    #[reflect(ignore)]
    pub transport: T,
    pub conditioner: Option<LinkConditionerConfig>,
    pub compression: CompressionConfig,
}

impl<T> SharedIoConfig<T> {
    pub fn from_transport(transport: T) -> Self {
        Self {
            transport,
            conditioner: None,
            compression: CompressionConfig::default(),
        }
    }
    pub fn with_conditioner(mut self, conditioner_config: LinkConditionerConfig) -> Self {
        self.conditioner = Some(conditioner_config);
        self
    }

    pub fn with_compression(mut self, compression_config: CompressionConfig) -> Self {
        self.compression = compression_config;
        self
    }
}
