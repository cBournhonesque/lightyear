use async_trait::async_trait;

use crate::Streams;

use super::maybe;

#[derive(Debug)]
pub struct Connecting<T>(pub T);

#[cfg_attr(not(target_family = "wasm"), async_trait)]
#[cfg_attr(target_family = "wasm", async_trait(?Send))]
impl<T> crate::traits::Connecting for Connecting<T>
where
    T: maybe::Send,
{
    type Connection = T;
    type Error = std::convert::Infallible;

    async fn wait_connect(self) -> Result<Self::Connection, Self::Error> {
        Ok(self.0)
    }
}

#[derive(Debug)]
pub struct OpeningUniStream<T: Streams>(pub T::SendStream);

#[cfg_attr(not(target_family = "wasm"), async_trait)]
#[cfg_attr(target_family = "wasm", async_trait(?Send))]
impl<T> crate::traits::OpeningUniStream for OpeningUniStream<T>
where
    T: Streams,
{
    type Streams = T;
    type Error = std::convert::Infallible;

    async fn wait_uni(self) -> Result<<T as Streams>::SendStream, Self::Error> {
        Ok(self.0)
    }
}

#[derive(Debug)]
pub struct OpeningBiStream<T: Streams>(pub (T::SendStream, T::RecvStream));

#[cfg_attr(not(target_family = "wasm"), async_trait)]
#[cfg_attr(target_family = "wasm", async_trait(?Send))]
impl<T> crate::traits::OpeningBiStream for OpeningBiStream<T>
where
    T: Streams,
{
    type Streams = T;
    type Error = std::convert::Infallible;

    async fn wait_bi(self) -> Result<crate::traits::BiStreamsFor<T>, Self::Error> {
        Ok(self.0)
    }
}
