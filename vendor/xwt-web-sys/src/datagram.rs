use std::{
    pin::Pin,
    task::{Context, Poll},
};

use super::reader::{ReadError, StreamReader};
use bytes::Bytes;
use futures::{ready, Future};
use parking_lot::Mutex;

use js_sys::Uint8Array;

/// Reads the next datagram from the connection
pub struct ReadDatagram<'a> {
    pub(crate) stream: &'a Mutex<StreamReader<Uint8Array>>,
}

impl Future for ReadDatagram<'_> {
    type Output = Option<Result<Bytes, ReadError>>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut datagrams = self.stream.lock();

        let data = ready!(datagrams.poll_next(cx));

        match data {
            Some(Ok(data)) => Poll::Ready(Some(Ok(data.to_vec().into()))),
            Some(Err(err)) => Poll::Ready(Some(Err(err))),
            None => Poll::Ready(None),
        }
    }
}
