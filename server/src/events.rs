use std::collections::HashMap;

use lightyear_shared::netcode::ClientId;
use lightyear_shared::{ConnectionEvents, Protocol};

pub struct ServerEvents<P: Protocol> {
    pub connections: Vec<ClientId>,
    pub disconnects: Vec<ClientId>,

    pub events: HashMap<ClientId, ConnectionEvents<P>>,
    pub empty: bool,
}

impl<P: Protocol> ServerEvents<P> {
    pub(crate) fn new() -> Self {
        Self {
            connections: Vec::new(),
            disconnects: Vec::new(),
            events: HashMap::new(),
            empty: true,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.empty
    }

    pub fn into_iter<V: for<'a> IterEvent<'a, P>>(&mut self) -> <V as IterEvent<'_, P>>::IntoIter {
        return V::into_iter(self);
    }

    pub fn iter<'a, V: IterEvent<'a, P>>(&'a self) -> V::Iter {
        return V::iter(self);
    }

    pub fn has<V: for<'a> IterEvent<'a, P>>(&self) -> bool {
        return V::has(self);
    }

    pub(crate) fn push_connections(&mut self, client_id: ClientId) {
        self.connections.push(client_id);
        self.empty = false;
    }

    pub(crate) fn push_disconnects(&mut self, client_id: ClientId) {
        self.disconnects.push(client_id);
        self.empty = false;
    }

    pub(crate) fn push_events(&mut self, client_id: ClientId, events: ConnectionEvents<P>) {
        if !events.is_empty() {
            self.events.insert(client_id, events);
            self.empty = false;
        }
    }
}

// TODO: this seems overly complicated for no reason
//  just write iter_connections(), etc.
pub trait IterEvent<'a, P: Protocol>
where
    <Self as IterEvent<'a, P>>::Item: 'a,
{
    type Item;
    type Iter: Iterator<Item = &'a Self::Item>;
    type IntoIter: Iterator<Item = Self::Item>;

    fn iter(events: &'a ServerEvents<P>) -> Self::Iter;
    fn into_iter(events: &mut ServerEvents<P>) -> Self::IntoIter;

    fn has(events: &ServerEvents<P>) -> bool;
}

pub struct ConnectEvent;

impl<'a, P: Protocol> IterEvent<'a, P> for ConnectEvent {
    type Item = ClientId;
    type Iter = std::slice::Iter<'a, ClientId>;
    type IntoIter = std::vec::IntoIter<ClientId>;

    fn iter(events: &'a ServerEvents<P>) -> Self::Iter {
        events.connections.iter()
    }

    fn into_iter(events: &mut ServerEvents<P>) -> Self::IntoIter {
        let list = std::mem::take(&mut events.connections);
        return IntoIterator::into_iter(list);
    }

    fn has(events: &ServerEvents<P>) -> bool {
        !events.connections.is_empty()
    }
}

pub struct DisconnectEvent;

impl<'a, P: Protocol> IterEvent<'a, P> for DisconnectEvent {
    type Item = ClientId;
    type Iter = std::slice::Iter<'a, ClientId>;
    type IntoIter = std::vec::IntoIter<ClientId>;

    fn iter(events: &'a ServerEvents<P>) -> Self::Iter {
        events.disconnects.iter()
    }

    fn into_iter(events: &mut ServerEvents<P>) -> Self::IntoIter {
        let list = std::mem::take(&mut events.disconnects);
        return IntoIterator::into_iter(list);
    }

    fn has(events: &ServerEvents<P>) -> bool {
        !events.disconnects.is_empty()
    }
}
