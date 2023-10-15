use std::collections::HashMap;

use lightyear_shared::netcode::ClientId;
use lightyear_shared::{Events, Protocol};

pub struct ServerEvents<P: Protocol> {
    pub events: HashMap<ClientId, Events<P>>,
}

impl<P: Protocol> ServerEvents<P> {
    pub(crate) fn new() -> Self {
        Self {
            events: HashMap::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    pub(crate) fn push_events(&mut self, client_id: ClientId, events: Events<P>) {
        self.events.insert(client_id, events);
    }
}
