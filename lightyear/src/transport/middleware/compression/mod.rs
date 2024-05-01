use bevy::prelude::Reflect;
use serde::{Deserialize, Serialize};

#[cfg(feature = "zstd")]
pub(crate) mod zstd;

#[derive(Clone, Copy, Debug, Default, Reflect, Serialize, Deserialize)]
pub enum CompressionConfig {
    #[default]
    None,
    #[cfg(feature = "zstd")]
    Zstd { level: i32 },
}
