use crate::{ConnectionEvents, Protocol};

pub struct ClientEvents<P: Protocol> {
    // cannot include connection/disconnection directly into ConnectionEvents, because we remove
    // the connection event upon disconnection
    connection: bool,
    disconnection: bool,
    events: ConnectionEvents<P>,
}
