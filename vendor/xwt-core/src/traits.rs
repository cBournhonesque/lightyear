use async_trait::async_trait;

use crate::{
    datagram,
    io::{Read, Write},
    utils::maybe,
};

pub trait Streams: maybe::Send {
    type SendStream: Write;
    type RecvStream: Read;
}

pub type BiStreamsFor<T> = (<T as Streams>::SendStream, <T as Streams>::RecvStream);

#[cfg_attr(not(target_family = "wasm"), async_trait)]
#[cfg_attr(target_family = "wasm", async_trait(?Send))]
pub trait OpeningBiStream: maybe::Send {
    type Streams: Streams;
    type Error: std::error::Error + maybe::Send + maybe::Sync + 'static;

    async fn wait_bi(self) -> Result<BiStreamsFor<Self::Streams>, Self::Error>;
}

#[cfg_attr(not(target_family = "wasm"), async_trait)]
#[cfg_attr(target_family = "wasm", async_trait(?Send))]
pub trait OpenBiStream: Streams {
    type Opening: OpeningBiStream<Streams = Self>;
    type Error: std::error::Error + maybe::Send + maybe::Sync + 'static;

    async fn open_bi(&self) -> Result<Self::Opening, Self::Error>;
}

#[cfg_attr(not(target_family = "wasm"), async_trait)]
#[cfg_attr(target_family = "wasm", async_trait(?Send))]
pub trait AcceptBiStream: Streams {
    type Error: std::error::Error + maybe::Send + maybe::Sync + 'static;

    async fn accept_bi(&self) -> Result<BiStreamsFor<Self>, Self::Error>;
}

#[cfg_attr(not(target_family = "wasm"), async_trait)]
#[cfg_attr(target_family = "wasm", async_trait(?Send))]
pub trait OpeningUniStream: maybe::Send {
    type Streams: Streams;
    type Error: std::error::Error + maybe::Send + maybe::Sync + 'static;

    async fn wait_uni(self) -> Result<<Self::Streams as Streams>::SendStream, Self::Error>;
}

#[cfg_attr(not(target_family = "wasm"), async_trait)]
#[cfg_attr(target_family = "wasm", async_trait(?Send))]
pub trait OpenUniStream: Streams {
    type Opening: OpeningUniStream;
    type Error: std::error::Error + maybe::Send + maybe::Sync + 'static;

    async fn open_uni(&self) -> Result<Self::Opening, Self::Error>;
}

#[cfg_attr(not(target_family = "wasm"), async_trait)]
#[cfg_attr(target_family = "wasm", async_trait(?Send))]
pub trait AcceptUniStream: Streams {
    type Error: std::error::Error + maybe::Send + maybe::Sync + 'static;

    async fn accept_uni(&self) -> Result<Self::RecvStream, Self::Error>;
}

#[cfg_attr(not(target_family = "wasm"), async_trait)]
#[cfg_attr(target_family = "wasm", async_trait(?Send))]
pub trait EndpointConnect: Sized + maybe::Send {
    type Connecting: Connecting;
    type Error: std::error::Error + maybe::Send + maybe::Sync + 'static;

    async fn connect(&self, url: &str) -> Result<Self::Connecting, Self::Error>;
}

#[cfg_attr(not(target_family = "wasm"), async_trait)]
#[cfg_attr(target_family = "wasm", async_trait(?Send))]
pub trait Connecting: maybe::Send {
    type Connection: maybe::Send;
    type Error: std::error::Error + maybe::Send + maybe::Sync + 'static;

    async fn wait_connect(self) -> Result<Self::Connection, Self::Error>;
}

#[cfg_attr(not(target_family = "wasm"), async_trait)]
#[cfg_attr(target_family = "wasm", async_trait(?Send))]
pub trait EndpointAccept: Sized + maybe::Send {
    type Accepting: Accepting;
    type Error: std::error::Error + maybe::Send + maybe::Sync + 'static;

    async fn accept(&self) -> Result<Option<Self::Accepting>, Self::Error>;
}

#[cfg_attr(not(target_family = "wasm"), async_trait)]
#[cfg_attr(target_family = "wasm", async_trait(?Send))]
pub trait Accepting: maybe::Send {
    type Request: Request;
    type Error: std::error::Error + maybe::Send + maybe::Sync + 'static;

    async fn wait_accept(self) -> Result<Self::Request, Self::Error>;
}

#[cfg_attr(not(target_family = "wasm"), async_trait)]
#[cfg_attr(target_family = "wasm", async_trait(?Send))]
pub trait Request: maybe::Send {
    type Connection: maybe::Send;
    type OkError: std::error::Error + maybe::Send + maybe::Sync + 'static;
    type CloseError: std::error::Error + maybe::Send + maybe::Sync + 'static;

    async fn ok(self) -> Result<Self::Connection, Self::OkError>;
    async fn close(self, status: u16) -> Result<(), Self::CloseError>;
}

pub trait Connection:
    Streams + OpenBiStream + OpenUniStream + AcceptBiStream + AcceptUniStream + datagram::Datagrams
{
}

impl<T> Connection for T where
    T: Streams
        + OpenBiStream
        + OpenUniStream
        + AcceptBiStream
        + AcceptUniStream
        + datagram::Datagrams
{
}
