/*! Defines a [`ConnectionEvents`] struct that is used to store all events that are received from a [`Connection`](crate::connection::Connection).
*/
use std::collections::HashMap;
use std::iter;

use bevy::prelude::{Component, Entity};
use tracing::{info, trace};

use crate::packet::message::Message;
use crate::prelude::Named;
use crate::protocol::channel::ChannelKind;
use crate::protocol::component::IntoKind;
use crate::protocol::message::{MessageBehaviour, MessageKind};
use crate::protocol::{EventContext, Protocol};
use crate::shared::ping::message::{Ping, Pong, SyncMessage};

// TODO: don't make fields pub but instead make accessors
#[derive(Debug)]
pub struct ConnectionEvents<P: Protocol> {
    // netcode
    // we put disconnections outside of there because `ConnectionEvents` gets removed upon disconnection
    pub connection: bool,

    // messages
    pub messages: HashMap<MessageKind, HashMap<ChannelKind, Vec<P::Message>>>,
    // replication
    pub spawns: Vec<Entity>,
    pub despawns: Vec<Entity>,
    // TODO: key by entity or by kind?
    // TODO: include the actual value in the event, or just the type? let's just include the type for now
    pub component_inserts: HashMap<P::ComponentKinds, Vec<Entity>>,
    // pub insert_components: HashMap<Entity, Vec<P::Components>>,
    pub component_removes: HashMap<P::ComponentKinds, Vec<Entity>>,
    // TODO: here as well, we could only include the type.. we already apply the changes to the entity directly, so users could keep track of changes
    //  let's just start with the kind...
    //  also, normally the updates are sequenced
    // TODO: include the tick for each update?
    pub component_updates: HashMap<P::ComponentKinds, Vec<Entity>>,
    empty: bool,
}

impl<P: Protocol> Default for ConnectionEvents<P> {
    fn default() -> Self {
        Self::new()
    }
}

impl<P: Protocol> ConnectionEvents<P> {
    pub fn new() -> Self {
        Self {
            // netcode
            connection: false,
            // inputs
            // inputs: InputBuffer::default(),
            // messages
            messages: HashMap::new(),
            // replication
            spawns: Vec::new(),
            despawns: Vec::new(),
            component_inserts: Default::default(),
            component_removes: Default::default(),
            component_updates: Default::default(),
            // bookkeeping
            empty: true,
        }
    }

    pub fn clear(&mut self) {
        self.connection = false;
        self.messages.clear();
        self.spawns.clear();
        self.despawns.clear();
        self.component_inserts.clear();
        self.component_removes.clear();
        self.component_updates.clear();
        self.empty = true;
    }

    /// If true, the connection was established
    pub fn has_connection(&self) -> bool {
        self.connection
    }

    pub fn push_connection(&mut self) {
        self.connection = true;
        self.empty = false;
    }

    pub fn is_empty(&self) -> bool {
        self.empty
    }
    pub fn push_message(&mut self, channel_kind: ChannelKind, message: P::Message) {
        trace!("Received message: {:?}", message.name());
        #[cfg(feature = "metrics")]
        {
            let message_name = message.name();
            metrics::increment_counter!("message", "kind" => message_name);
        }
        self.messages
            .entry(message.kind())
            .or_default()
            .entry(channel_kind)
            .or_default()
            .push(message);
        self.empty = false;
    }

    pub(crate) fn push_spawn(&mut self, entity: Entity) {
        trace!(?entity, "Received entity spawn");
        #[cfg(feature = "metrics")]
        {
            metrics::increment_counter!("entity_spawn");
        }
        self.spawns.push(entity);
        self.empty = false;
    }

    pub(crate) fn push_despawn(&mut self, entity: Entity) {
        trace!(?entity, "Received entity despawn");
        #[cfg(feature = "metrics")]
        {
            metrics::increment_counter!("entity_despawn");
        }
        self.despawns.push(entity);
        self.empty = false;
    }

    pub(crate) fn push_insert_component(&mut self, entity: Entity, component: P::ComponentKinds) {
        trace!(?entity, ?component, "Received insert component");
        #[cfg(feature = "metrics")]
        {
            metrics::increment_counter!("component_insert", "kind" => component);
        }
        self.component_inserts
            .entry(component)
            .or_default()
            .push(entity);
        self.empty = false;
    }

    pub(crate) fn push_remove_component(&mut self, entity: Entity, component: P::ComponentKinds) {
        trace!(?entity, ?component, "Received remove component");
        #[cfg(feature = "metrics")]
        {
            metrics::increment_counter!("component_remove", "kind" => component);
        }
        self.component_removes
            .entry(component)
            .or_default()
            .push(entity);
        self.empty = false;
    }

    // TODO: how do distinguish between multiple updates for the same component/entity? add ticks?
    pub(crate) fn push_update_component(&mut self, entity: Entity, component: P::ComponentKinds) {
        trace!(?entity, ?component, "Received update component");
        #[cfg(feature = "metrics")]
        {
            metrics::increment_counter!("component_update", "kind" => component);
        }
        self.component_updates
            .entry(component)
            .or_default()
            .push(entity);
        self.empty = false;
    }
}

pub trait IterMessageEvent<P: Protocol, Ctx: EventContext = ()> {
    fn into_iter_messages<M: Message>(&mut self) -> Box<dyn Iterator<Item = (M, Ctx)> + '_>
    where
        P::Message: TryInto<M, Error = ()>;

    fn has_messages<M: Message>(&self) -> bool;
}

impl<P: Protocol> IterMessageEvent<P> for ConnectionEvents<P> {
    fn into_iter_messages<M: Message>(&mut self) -> Box<dyn Iterator<Item = (M, ())>>
    where
        // TODO: should we change this to `Into`
        P::Message: TryInto<M, Error = ()>,
    {
        let message_kind = MessageKind::of::<M>();
        if let Some(data) = self.messages.remove(&message_kind) {
            return Box::new(data.into_iter().flat_map(|(_, messages)| {
                messages.into_iter().map(|message| {
                    // SAFETY: we checked via message kind that only messages of the type M
                    // are in the list
                    (message.try_into().unwrap(), ())
                })
            }));
        }
        Box::new(iter::empty())
    }

    fn has_messages<M: Message>(&self) -> bool {
        let message_kind = MessageKind::of::<M>();
        self.messages.contains_key(&message_kind)
    }
}

pub trait IterEntitySpawnEvent<Ctx: EventContext = ()> {
    fn into_iter_entity_spawn(&mut self) -> Box<dyn Iterator<Item = (Entity, Ctx)> + '_>;
    fn has_entity_spawn(&self) -> bool;
}

impl<P: Protocol> IterEntitySpawnEvent for ConnectionEvents<P> {
    fn into_iter_entity_spawn(&mut self) -> Box<dyn Iterator<Item = (Entity, ())> + '_> {
        let spawns = std::mem::take(&mut self.spawns);
        Box::new(spawns.into_iter().map(|entity| (entity, ())))
    }

    fn has_entity_spawn(&self) -> bool {
        !self.spawns.is_empty()
    }
}

pub trait IterEntityDespawnEvent<Ctx: EventContext = ()> {
    fn into_iter_entity_despawn(&mut self) -> Box<dyn Iterator<Item = (Entity, Ctx)> + '_>;
    fn has_entity_despawn(&self) -> bool;
}

impl<P: Protocol> IterEntityDespawnEvent for ConnectionEvents<P> {
    fn into_iter_entity_despawn(&mut self) -> Box<dyn Iterator<Item = (Entity, ())> + '_> {
        let despawns = std::mem::take(&mut self.despawns);
        Box::new(despawns.into_iter().map(|entity| (entity, ())))
    }

    fn has_entity_despawn(&self) -> bool {
        !self.despawns.is_empty()
    }
}

pub trait IterComponentUpdateEvent<P: Protocol, Ctx: EventContext = ()> {
    fn into_iter_component_update<C: Component>(
        &mut self,
    ) -> Box<dyn Iterator<Item = (Entity, Ctx)> + '_>
    where
        C: IntoKind<P::ComponentKinds>;
    fn has_component_update<C: Component>(&self) -> bool
    where
        C: IntoKind<P::ComponentKinds>;
}

impl<P: Protocol> IterComponentUpdateEvent<P> for ConnectionEvents<P> {
    fn into_iter_component_update<C: Component>(
        &mut self,
    ) -> Box<dyn Iterator<Item = (Entity, ())> + '_>
    where
        C: IntoKind<P::ComponentKinds>,
    {
        let component_kind = C::into_kind();
        if let Some(data) = self.component_updates.remove(&component_kind) {
            return Box::new(data.into_iter().map(|entity| (entity, ())));
        }
        Box::new(iter::empty())
    }

    fn has_component_update<C: Component>(&self) -> bool
    where
        C: IntoKind<P::ComponentKinds>,
    {
        let component_kind = C::into_kind();
        self.component_updates.contains_key(&component_kind)
    }
}

pub trait IterComponentRemoveEvent<P: Protocol, Ctx: EventContext = ()> {
    fn into_iter_component_remove<C: Component>(
        &mut self,
    ) -> Box<dyn Iterator<Item = (Entity, Ctx)> + '_>
    where
        C: IntoKind<P::ComponentKinds>;
    fn has_component_remove<C: Component>(&self) -> bool
    where
        C: IntoKind<P::ComponentKinds>;
}

impl<P: Protocol> IterComponentRemoveEvent<P> for ConnectionEvents<P> {
    fn into_iter_component_remove<C: Component>(
        &mut self,
    ) -> Box<dyn Iterator<Item = (Entity, ())> + '_>
    where
        C: IntoKind<P::ComponentKinds>,
    {
        let component_kind = C::into_kind();
        if let Some(data) = self.component_removes.remove(&component_kind) {
            return Box::new(data.into_iter().map(|entity| (entity, ())));
        }
        Box::new(iter::empty())
    }

    fn has_component_remove<C: Component>(&self) -> bool
    where
        C: IntoKind<P::ComponentKinds>,
    {
        let component_kind = C::into_kind();
        self.component_removes.contains_key(&component_kind)
    }
}

pub trait IterComponentInsertEvent<P: Protocol, Ctx: EventContext = ()> {
    fn into_iter_component_insert<C: Component>(
        &mut self,
    ) -> Box<dyn Iterator<Item = (Entity, Ctx)> + '_>
    where
        C: IntoKind<P::ComponentKinds>;
    fn has_component_insert<C: Component>(&self) -> bool
    where
        C: IntoKind<P::ComponentKinds>;
}

impl<P: Protocol> IterComponentInsertEvent<P> for ConnectionEvents<P> {
    fn into_iter_component_insert<C: Component>(
        &mut self,
    ) -> Box<dyn Iterator<Item = (Entity, ())> + '_>
    where
        C: IntoKind<P::ComponentKinds>,
    {
        let component_kind = C::into_kind();
        if let Some(data) = self.component_inserts.remove(&component_kind) {
            return Box::new(data.into_iter().map(|entity| (entity, ())));
        }
        Box::new(iter::empty())
    }

    fn has_component_insert<C: Component>(&self) -> bool
    where
        C: IntoKind<P::ComponentKinds>,
    {
        let component_kind = C::into_kind();
        self.component_inserts.contains_key(&component_kind)
    }
}

#[cfg(test)]
mod tests {
    use crate::tests::protocol::{
        Channel1, Channel2, Message1, Message2, MyMessageProtocol, MyProtocol,
    };

    use super::*;

    #[test]
    fn test_iter_messages() {
        let mut events = ConnectionEvents::<MyProtocol>::new();
        let channel_kind_1 = ChannelKind::of::<Channel1>();
        let channel_kind_2 = ChannelKind::of::<Channel2>();
        let message1_a = Message1("hello".to_string());
        let message1_b = Message1("world".to_string());
        events.push_message(
            channel_kind_1,
            MyMessageProtocol::Message1(message1_a.clone()),
        );
        events.push_message(
            channel_kind_2,
            MyMessageProtocol::Message1(message1_b.clone()),
        );
        events.push_message(channel_kind_1, MyMessageProtocol::Message2(Message2(1)));

        // check that we have the correct messages
        let messages: Vec<Message1> = events.into_iter_messages().map(|(m, _)| m).collect();
        assert!(messages.contains(&message1_a));
        assert!(messages.contains(&message1_b));

        // check that there are no more message of that kind in the events
        assert!(!events.messages.contains_key(&MessageKind::of::<Message1>()));

        // check that we still have the other message kinds
        assert!(events.messages.contains_key(&MessageKind::of::<Message2>()));
    }
}
