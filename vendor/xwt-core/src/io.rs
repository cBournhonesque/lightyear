use async_trait::async_trait;

use crate::utils::maybe;

#[cfg_attr(not(target_family = "wasm"), async_trait)]
#[cfg_attr(target_family = "wasm", async_trait(?Send))]
pub trait Read: maybe::Send {
    type Error: std::error::Error + maybe::Send + maybe::Sync + 'static;

    async fn read(&mut self, buf: &mut [u8]) -> Result<Option<usize>, Self::Error>;
}

#[cfg_attr(not(target_family = "wasm"), async_trait)]
#[cfg_attr(target_family = "wasm", async_trait(?Send))]
pub trait Write: maybe::Send {
    type Error: std::error::Error + maybe::Send + maybe::Sync + 'static;

    async fn write(&mut self, buf: &[u8]) -> Result<usize, Self::Error>;
}

#[derive(Debug)]
pub struct Chunk<Data> {
    pub offset: u64,
    pub data: Data,
}

#[cfg_attr(not(target_family = "wasm"), async_trait)]
#[cfg_attr(target_family = "wasm", async_trait(?Send))]
pub trait ReadChunk<ChunkType: ?Sized + ReadableChunk>: maybe::Send {
    type Error: std::error::Error + maybe::Send + maybe::Sync + 'static;

    async fn read_chunk(
        &mut self,
        max_length: usize,
        ordered: bool,
    ) -> Result<Option<Chunk<<ChunkType as ReadableChunk>::Data<'_>>>, Self::Error>;
}

#[cfg_attr(not(target_family = "wasm"), async_trait)]
#[cfg_attr(target_family = "wasm", async_trait(?Send))]
pub trait WriteChunk<ChunkType: ?Sized + WriteableChunk>: maybe::Send {
    type Error: std::error::Error + maybe::Send + maybe::Sync + 'static;

    async fn write_chunk(
        &mut self,
        buf: <ChunkType as WriteableChunk>::Data<'_>,
    ) -> Result<(), Self::Error>;
}

#[cfg_attr(not(target_family = "wasm"), async_trait)]
#[cfg_attr(target_family = "wasm", async_trait(?Send))]
pub trait ReadableChunk: maybe::Send {
    type Data<'a>: AsRef<[u8]>;
}

#[cfg_attr(not(target_family = "wasm"), async_trait)]
#[cfg_attr(target_family = "wasm", async_trait(?Send))]
pub trait WriteableChunk: maybe::Send {
    type Data<'a>: From<&'a [u8]>;
}

pub mod chunk {
    use super::*;

    /// A chunk type that represents operations that carry the data as [`u8`]
    /// [`slice`]s or [`Vec`]s
    #[derive(Debug)]
    pub struct U8;

    impl WriteableChunk for U8 {
        type Data<'b> = &'b [u8];
    }

    impl ReadableChunk for U8 {
        type Data<'b> = Vec<u8>;
    }
}
