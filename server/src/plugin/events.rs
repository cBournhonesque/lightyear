use lightyear_shared::{ClientId, Message};

pub struct MessageEvent<M: Message> {
    inner: M,
    client_id: ClientId,
}

impl<M: Message> MessageEvent<M> {
    pub fn new(inner: M, client_id: ClientId) -> Self {
        Self { inner, client_id }
    }
}
