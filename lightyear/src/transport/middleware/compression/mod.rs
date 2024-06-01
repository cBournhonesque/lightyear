use bevy::prelude::Reflect;
use serde::{Deserialize, Serialize};

#[cfg(feature = "zstd")]
pub(crate) mod zstd;

#[cfg(feature = "lz4")]
pub(crate) mod lz4;

#[derive(Clone, Copy, Debug, Default, Reflect, Serialize, Deserialize)]
pub enum CompressionConfig {
    #[default]
    None,
    #[cfg(feature = "zstd")]
    Zstd { level: i32 },
    #[cfg(feature = "lz4")]
    Lz4,
}
