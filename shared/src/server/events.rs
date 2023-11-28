use std::collections::HashMap;

use bevy::prelude::{Component, Entity};

use crate::connection::events::{ConnectionEvents, IterEntitySpawnEvent, IterMessageEvent};
use crate::netcode::ClientId;
use crate::packet::message::Message;
use crate::protocol::Protocol;
use crate::shared::ping::message::{Ping, Pong};

pub struct ServerEvents<P: Protocol> {
    // have to handle disconnects separately because the [`ConnectionEvents`] are removed upon disconnection
    pub disconnects: Vec<ClientId>,
    pub events: HashMap<ClientId, ConnectionEvents<P>>,
    pub empty: bool,
}

impl<P: Protocol> ServerEvents<P> {
    pub(crate) fn new() -> Self {
        Self {
            disconnects: Vec::new(),
            events: HashMap::new(),
            empty: true,
        }
    }

    /// Clear all events except for the input buffer which we want to keep around
    // TODO: this seems to show that events might not the right place to put the input buffer
    //  maybe we want to create a dedicated InputBuffer resource for it?
    //  on server-side it would be hashmap, and we need to sync it with connections/disconnections
    pub(crate) fn clear(&mut self) {
        self.disconnects = Vec::new();
        self.empty = true;
        self.events.values_mut().for_each(|events| events.clear());
    }

    pub fn is_empty(&self) -> bool {
        self.empty
    }

    // TODO: could also return a IntoIterMessages struct and impl Iterator for that

    // TODO: seems like we cannot chain iterators like this; because then we need to keep &mut Self around
    //  instead we want to take the contents
    // pub fn into_iter_messages<M: Message>(&mut self) -> impl Iterator<Item = (M, ClientId)> + '_
    // where
    //     P::Message: TryInto<M, Error = ()>,
    // {
    //     self.events.iter_mut().flat_map(|(client_id, events)| {
    //         let messages = events.into_iter_messages::<M>();
    //         let client_ids = std::iter::once(client_id.clone()).cycle();
    //         return messages.zip(client_ids);
    //     })
    // }
    //
    // pub fn has_messages<M: Message>(&mut self) -> bool {
    //     self.events
    //         .iter()
    //         .any(|(_, connection_events)| connection_events.has_messages::<M>())
    // }

    // TODO: should we consume connections?
    pub fn iter_connections(&self) -> impl Iterator<Item = ClientId> + '_ {
        self.events
            .iter()
            .filter_map(|(client_id, events)| events.has_connection().then_some(*client_id))
    }

    pub fn has_connections(&self) -> bool {
        self.events
            .iter()
            .any(|(_, connection_events)| connection_events.has_connection())
    }

    // /// Pop the inputs for all clients for the given tick
    // pub fn pop_inputs(
    //     &mut self,
    //     tick: Tick,
    // ) -> impl Iterator<Item = (Option<P::Input>, ClientId)> + '_ {
    //     self.events.iter_mut().map(move |(client_id, events)| {
    //         let input = events.pop_input(tick);
    //         (input, client_id.clone())
    //     })
    // }
    //
    // /// Get the inputs for all clients for the given tick
    // pub fn get_inputs(
    //     &mut self,
    //     tick: Tick,
    // ) -> impl Iterator<Item = (Option<&P::Input>, ClientId)> + '_ {
    //     self.events.iter_mut().map(move |(client_id, events)| {
    //         let input = events.get_input(tick);
    //         (input, client_id.clone())
    //     })
    // }

    pub fn iter_disconnections(&self) -> impl Iterator<Item = ClientId> + '_ {
        self.events
            .iter()
            .filter_map(|(client_id, events)| events.has_disconnection().then_some(*client_id))
    }

    pub fn has_disconnections(&self) -> bool {
        self.events
            .iter()
            .any(|(_, connection_events)| connection_events.has_disconnection())
    }

    // pub fn into_iter<V: for<'a> IterEvent<'a, P>>(&mut self) -> <V as IterEvent<'_, P>>::IntoIter {
    //     return V::into_iter(self);
    // }
    //
    // pub fn iter<'a, V: IterEvent<'a, P>>(&'a self) -> V::Iter {
    //     return V::iter(self);
    // }
    //
    // pub fn has<V: for<'a> IterEvent<'a, P>>(&self) -> bool {
    //     return V::has(self);
    // }

    // Cannot only use the 'disconnect' field in the events, because we remove the events
    // upon disconnection
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

impl<P: Protocol> IterMessageEvent<P, ClientId> for ServerEvents<P> {
    fn into_iter_messages<M: Message>(&mut self) -> Box<dyn Iterator<Item = (M, ClientId)> + '_>
    where
        P::Message: TryInto<M, Error = ()>,
    {
        Box::new(self.events.iter_mut().flat_map(|(client_id, events)| {
            let messages = events.into_iter_messages::<M>().map(|(message, _)| message);
            let client_ids = std::iter::once(*client_id).cycle();
            messages.zip(client_ids)
        }))
    }

    fn has_messages<M: Message>(&self) -> bool {
        self.events
            .iter()
            .any(|(_, connection_events)| connection_events.has_messages::<M>())
    }
}

impl<P: Protocol> IterEntitySpawnEvent<ClientId> for ServerEvents<P> {
    fn into_iter_entity_spawn(&mut self) -> Box<dyn Iterator<Item = (Entity, ClientId)> + '_> {
        Box::new(self.events.iter_mut().flat_map(|(client_id, events)| {
            let entities = events.into_iter_entity_spawn().map(|(entity, _)| entity);
            let client_ids = std::iter::once(*client_id).cycle();
            entities.zip(client_ids)
        }))
    }

    fn has_entity_spawn(&self) -> bool {
        self.events
            .iter()
            .any(|(_, connection_events)| connection_events.has_entity_spawn())
    }
}

pub type ConnectEvent = crate::shared::events::ConnectEvent<ClientId>;
pub type DisconnectEvent = crate::shared::events::DisconnectEvent<ClientId>;
pub type InputEvent<I> = crate::shared::events::InputEvent<I, ClientId>;
pub type EntitySpawnEvent = crate::shared::events::EntitySpawnEvent<ClientId>;
pub type EntityDespawnEvent = crate::shared::events::EntityDespawnEvent<ClientId>;
pub type ComponentUpdateEvent<C> = crate::shared::events::ComponentUpdateEvent<C, ClientId>;
pub type ComponentInsertEvent<C> = crate::shared::events::ComponentInsertEvent<C, ClientId>;
pub type ComponentRemoveEvent<C> = crate::shared::events::ComponentRemoveEvent<C, ClientId>;
pub type MessageEvent<M> = crate::shared::events::MessageEvent<M, ClientId>;

#[cfg(test)]
mod tests {
    use crate::protocol::channel::ChannelKind;
    use crate::protocol::tests::{
        Channel1, Channel2, Message1, Message2, MyMessageProtocol, MyProtocol,
    };

    use super::*;

    #[test]
    fn test_iter_messages() {
        let mut events_1 = ConnectionEvents::<MyProtocol>::new();
        let channel_kind_1 = ChannelKind::of::<Channel1>();
        let channel_kind_2 = ChannelKind::of::<Channel2>();
        let message1_a = Message1("hello".to_string());
        let message1_b = Message1("world".to_string());
        events_1.push_message(
            channel_kind_1,
            MyMessageProtocol::Message1(message1_a.clone()),
        );
        events_1.push_message(
            channel_kind_2,
            MyMessageProtocol::Message1(message1_b.clone()),
        );
        events_1.push_message(channel_kind_1, MyMessageProtocol::Message2(Message2(1)));

        let mut server_events = ServerEvents::<MyProtocol>::new();
        server_events.push_events(1, events_1);

        let mut events_2 = ConnectionEvents::<MyProtocol>::new();
        let message1_c = Message1("test".to_string());
        events_2.push_message(
            channel_kind_1,
            MyMessageProtocol::Message1(message1_c.clone()),
        );
        events_2.push_message(channel_kind_1, MyMessageProtocol::Message2(Message2(2)));

        server_events.push_events(2, events_2);

        // check that we have the correct messages
        let messages: Vec<(Message1, ClientId)> = server_events.into_iter_messages().collect();
        assert_eq!(messages.len(), 3);
        assert!(messages.contains(&(message1_a, 1)));
        assert!(messages.contains(&(message1_b, 1)));
        assert!(messages.contains(&(message1_c, 2)));

        // check that there are no more message of that kind in the events
        assert!(!server_events
            .events
            .get(&1)
            .unwrap()
            .has_messages::<Message1>());
        assert!(!server_events
            .events
            .get(&2)
            .unwrap()
            .has_messages::<Message1>());

        // check that we still have the other message kinds
        assert!(server_events
            .events
            .get(&2)
            .unwrap()
            .has_messages::<Message2>());
    }
}
