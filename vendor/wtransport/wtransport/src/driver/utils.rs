use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::sync::watch;
use tokio::sync::Mutex;
use wtransport_proto::ids::StreamId;
use wtransport_proto::varint::VarInt;

#[inline(always)]
pub fn varint_q2w(varint: quinn::VarInt) -> VarInt {
    // SAFETY: varint conversion
    unsafe {
        debug_assert!(varint.into_inner() <= VarInt::MAX.into_inner());
        VarInt::from_u64_unchecked(varint.into_inner())
    }
}

#[inline(always)]
pub fn varint_w2q(varint: VarInt) -> quinn::VarInt {
    // SAFETY: varint conversion
    unsafe {
        debug_assert!(varint.into_inner() <= quinn::VarInt::MAX.into_inner());
        quinn::VarInt::from_u64_unchecked(varint.into_inner())
    }
}

#[inline(always)]
pub fn streamid_q2w(stream_id: quinn::StreamId) -> StreamId {
    let varint = unsafe {
        debug_assert!(stream_id.0 <= VarInt::MAX.into_inner());
        VarInt::from_u64_unchecked(stream_id.0)
    };

    StreamId::new(varint)
}

pub fn shared_result<T>() -> (SharedResultSet<T>, SharedResultGet<T>)
where
    T: Copy,
{
    let set = SharedResultSet::new();
    let get = set.subscribe();
    (set, get)
}

#[derive(Debug, Clone)]
pub struct SharedResultSet<T>(Arc<watch::Sender<Option<T>>>);

impl<T> SharedResultSet<T>
where
    T: Copy,
{
    #[inline(always)]
    pub fn new() -> Self {
        Self(Arc::new(watch::channel(None).0))
    }

    /// Sets the shared result in thread safe manner.
    ///
    /// The first call will be able to actually set the inner value,
    /// successive calls end up into being no-op.
    ///
    /// Returns `true` if the inner result is actually set.
    pub fn set(&self, result: T) -> bool {
        self.0.send_if_modified(|state| {
            if state.is_none() {
                *state = Some(result);
                true
            } else {
                false
            }
        })
    }

    /// Awaits all subscribers are dead.
    #[inline(always)]
    pub async fn closed(&self) {
        self.0.closed().await;
    }

    /// Generates a new subscriber.
    ///
    /// A subscriber is able to be notified when the shared result
    /// will be set.
    #[inline(always)]
    pub fn subscribe(&self) -> SharedResultGet<T> {
        SharedResultGet(Mutex::new(self.0.subscribe()))
    }
}

#[derive(Debug)]
pub struct SharedResultGet<T>(Mutex<watch::Receiver<Option<T>>>);

impl<T> SharedResultGet<T>
where
    T: Copy,
{
    /// Awaits the shared result is set by any setter.
    ///
    /// Once the shared result is set, this method will always
    /// return that value (i.e., `Some(T)`).
    ///
    /// If all setters are dead before setting any result, this will
    /// return `None`. And all successive calls will return `None`.
    pub async fn result(&self) -> Option<T> {
        let mut lock = self.0.lock().await;

        loop {
            if let Some(result) = *lock.borrow() {
                return Some(result);
            }

            if lock.changed().await.is_err() {
                return None;
            }
        }
    }
}

pub struct SendError;

pub enum TrySendError<T> {
    Full(T),
    Closed(T),
}

#[derive(Debug)]
pub struct BiChannelEndpoint<T> {
    sender: mpsc::Sender<T>,
    receiver: Mutex<mpsc::Receiver<T>>,
}

impl<T> BiChannelEndpoint<T> {
    #[inline(always)]
    pub async fn send(&self, value: T) -> Result<(), SendError> {
        self.sender.send(value).await.map_err(|_| SendError)
    }

    pub fn try_send(&self, value: T) -> Result<(), TrySendError<T>> {
        self.sender.try_send(value).map_err(|error| match error {
            mpsc::error::TrySendError::Full(value) => TrySendError::Full(value),
            mpsc::error::TrySendError::Closed(value) => TrySendError::Closed(value),
        })
    }

    #[inline(always)]
    pub async fn recv(&self) -> Option<T> {
        self.receiver.lock().await.recv().await
    }
}

pub fn bichannel<T>(capacity: usize) -> (BiChannelEndpoint<T>, BiChannelEndpoint<T>) {
    let c1 = mpsc::channel(capacity);
    let c2 = mpsc::channel(capacity);

    (
        BiChannelEndpoint {
            sender: c1.0,
            receiver: Mutex::new(c2.1),
        },
        BiChannelEndpoint {
            sender: c2.0,
            receiver: Mutex::new(c1.1),
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use utils::poll_once;

    #[tokio::test]
    async fn shared_result_double_set() {
        let set = SharedResultSet::new();
        assert!(set.set(1));
        assert!(!set.set(2));

        let get = set.subscribe();
        assert!(matches!(get.result().await, Some(1)));
    }

    #[tokio::test]
    async fn shared_result_get_drop() {
        let set = SharedResultSet::<()>::new();
        let get = set.subscribe();
        drop(set);
        assert!(get.result().await.is_none());
        assert!(get.result().await.is_none());
    }

    #[tokio::test]
    async fn shared_result_get() {
        let set = SharedResultSet::new();
        let get = set.subscribe();

        assert!(poll_once(get.result()).await.is_none());

        set.set(1);
        drop(set);

        assert!(matches!(poll_once(get.result()).await.unwrap(), Some(1)));
        assert!(matches!(poll_once(get.result()).await.unwrap(), Some(1)));
    }

    mod utils {
        use std::future::Future;
        use std::pin::Pin;
        use std::task::Context;
        use std::task::Poll;

        pub async fn poll_once<F, T>(future: F) -> Option<T>
        where
            F: Future<Output = T>,
        {
            PollOnce(Box::pin(future)).await
        }

        struct PollOnce<F>(Pin<Box<F>>);

        impl<F, T> Future for PollOnce<F>
        where
            F: Future<Output = T>,
        {
            type Output = Option<T>;

            fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
                match Future::poll(self.0.as_mut(), cx) {
                    Poll::Ready(result) => Poll::Ready(Some(result)),
                    Poll::Pending => Poll::Ready(None),
                }
            }
        }
    }
}
