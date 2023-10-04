use crate::clients::ClientId;

pub enum Event {
    Connection(ClientId),
}
