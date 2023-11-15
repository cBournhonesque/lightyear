use std::collections::HashMap;
use std::iter;

use crate::netcode::ClientId;
use bevy::prelude::{Component, Entity, Event};
use tracing::trace;

use crate::inputs::input_buffer::InputBuffer;
use crate::protocol::message::MessageKind;
use crate::tick::Tick;
use crate::{
    Channel, ChannelKind, InputMessage, IntoKind, Message, MessageBehaviour, PingMessage,
    PongMessage, Protocol, SyncMessage,
};

// TODO: don't make fields pub but instead make accessors
#[derive(Debug)]
pub struct ConnectionEvents<P: Protocol> {
    // netcode
    pub connection: bool,
    pub disconnection: bool,

    // sync
    pub pings: Vec<PingMessage>,
    pub pongs: Vec<PongMessage>,
    pub syncs: Vec<SyncMessage>,
    // inputs
    // // TODO: maybe support a vec of inputs?
    // // TODO: we put the InputBuffer here right now instead of Connection because this struct is the one that is the most
    // //  accessible from bevy. Maybe refactor later
    // //  THIS ONLY CONTAINS THE INPUTS RECEIVED FROM REMOTE, I.E THIS FIELD IS ONLY USED BY THE SERVER RIGHT NOW
    // pub inputs: InputBuffer<P::Input>,
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
            disconnection: false,
            // sync
            pings: Vec::new(),
            pongs: Vec::new(),
            syncs: Vec::new(),
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

    // TODO: this seems to show that events might not the right place to put the input buffer
    //  maybe we want to create a dedicated InputBuffer resource for it?
    //  on server-side it would be hashmap, and we need to sync it with connections/disconnections
    pub fn clear(&mut self) {
        self.connection = false;
        self.disconnection = false;
        self.pings.clear();
        self.pongs.clear();
        self.syncs.clear();
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

    pub fn has_disconnection(&self) -> bool {
        self.disconnection
    }

    pub fn push_disconnection(&mut self) {
        self.disconnection = true;
        self.empty = false;
    }

    pub fn has_syncs(&self) -> bool {
        !self.syncs.is_empty()
    }

    pub fn push_sync(&mut self, sync: SyncMessage) {
        self.syncs.push(sync);
        self.empty = false;
    }

    pub fn into_iter_syncs(&mut self) -> impl Iterator<Item = SyncMessage> + '_ {
        std::mem::take(&mut self.syncs).into_iter()
    }

    pub fn has_pings(&self) -> bool {
        !self.pings.is_empty()
    }

    pub fn push_ping(&mut self, ping: PingMessage) {
        self.pings.push(ping);
        self.empty = false;
    }

    pub fn into_iter_pings(&mut self) -> impl Iterator<Item = PingMessage> + '_ {
        std::mem::take(&mut self.pings).into_iter()
    }

    pub fn has_pongs(&self) -> bool {
        !self.pongs.is_empty()
    }

    pub fn push_pong(&mut self, pong: PongMessage) {
        self.pongs.push(pong);
        self.empty = false;
    }

    pub fn into_iter_pongs(&mut self) -> impl Iterator<Item = PongMessage> + '_ {
        std::mem::take(&mut self.pongs).into_iter()
    }

    // /// Pop the input for the current tick from the input buffer
    // /// We can pop it because we won't be needing it anymore?
    // /// Maybe not because of rollback!
    // pub fn pop_input(&mut self, tick: Tick) -> Option<P::Input> {
    //     self.inputs.buffer.remove(&tick)
    // }
    //
    // pub fn get_input(&mut self, tick: Tick) -> Option<&P::Input> {
    //     self.inputs.buffer.get(&tick)
    // }

    pub fn is_empty(&self) -> bool {
        self.empty
    }
    pub fn push_message(&mut self, channel_kind: ChannelKind, message: P::Message) {
        self.messages
            .entry(message.kind())
            .or_default()
            .entry(channel_kind)
            .or_default()
            .push(message);
        self.empty = false;
    }

    pub(crate) fn push_spawn(&mut self, entity: Entity) {
        self.spawns.push(entity);
        self.empty = false;
    }

    pub(crate) fn push_despawn(&mut self, entity: Entity) {
        self.despawns.push(entity);
        self.empty = false;
    }

    pub(crate) fn push_insert_component(&mut self, entity: Entity, component: P::ComponentKinds) {
        self.component_inserts
            .entry(component)
            .or_default()
            .push(entity);
        self.empty = false;
    }

    pub(crate) fn push_remove_component(&mut self, entity: Entity, component: P::ComponentKinds) {
        self.component_removes
            .entry(component)
            .or_default()
            .push(entity);
        self.empty = false;
    }

    // TODO: how do distinguish between multiple updates for the same component/entity? add ticks?
    pub(crate) fn push_update_component(&mut self, entity: Entity, component: P::ComponentKinds) {
        self.component_updates
            .entry(component)
            .or_default()
            .push(entity);
        self.empty = false;
    }
}

/// Data that can be used in an Event
/// Same as `Event`, but we implement it automatically for all compatible types
pub trait EventContext: Send + Sync + 'static {}

impl<T: Send + Sync + 'static> EventContext for T {}

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
        return Box::new(iter::empty());
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
        return Box::new(iter::empty());
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
        return Box::new(iter::empty());
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
        return Box::new(iter::empty());
    }

    fn has_component_insert<C: Component>(&self) -> bool
    where
        C: IntoKind<P::ComponentKinds>,
    {
        let component_kind = C::into_kind();
        self.component_inserts.contains_key(&component_kind)
    }
}

// pub trait IterMessageEvent<M: Message, P: Protocol, Ctx = ()>
// where
//     P::Message: TryInto<M, Error = ()>,
// {
//     fn into_iter_messages(&mut self) -> Box<dyn Iterator<Item = (M, Ctx)>>;
//
//     fn has_messages(&self) -> bool;
// }

// impl<M: Message, P: Protocol> IterMessageEvent<M, P> for ConnectionEvents<P> {
//     fn into_iter_messages(&mut self) -> Box<dyn Iterator<Item = (M, ())>> {
//         let message_kind = MessageKind::of::<M>();
//         if let Some(data) = self.messages.remove(&message_kind) {
//             return Box::new(data.into_iter().flat_map(|data| {
//                 data.into_iter().flat_map(|(_, messages)| {
//                     messages.into_iter().map(|message| {
//                         // SAFETY: we checked via message kind that only messages of the type M
//                         // are in the list
//                         (message.try_into().unwrap(), ())
//                     })
//                 })
//             }));
//         }
//         return Box::new(iter::empty());
//     }
//
//     fn has_messages(&self) -> bool {
//         let message_kind = MessageKind::of::<M>();
//         self.messages.contains_key(&message_kind)
//     }
// }
//
// pub trait IterConnectionEvent<P: Protocol> {
//     type Iter;
//     type IntoIter;
//
//     fn iter(events: &mut ConnectionEvents<P>) -> Self::Iter;
//
//     fn has(events: &ConnectionEvents<P>) -> bool;
// }

// pub struct MessageEvent<C: Channel, M: Message> {
//     _phantom: std::marker::PhantomData<(C, M)>,
// }
//
// impl<P: Protocol, C> IterEvent<P> for MessageEvent<, M> {
//     type Iter = IntoIter<M>;
//
//     fn iter(events: &mut Events<E>) -> Self::Iter {
//         let channel_kind: ChannelKind = ChannelKind::of::<C>();
//         if let Some(channel_map) = events.messages.get_mut(&channel_kind) {
//             let message_kind: MessageKind = MessageKind::of::<M>();
//             if let Some(boxed_list) = channel_map.remove(&message_kind) {
//                 let mut output_list: Vec<M> = Vec::new();
//
//                 for boxed_message in boxed_list {
//                     let boxed_any = boxed_message.to_boxed_any();
//                     let message = boxed_any.downcast::<M>().unwrap();
//                     output_list.push(*message);
//                 }
//
//                 return IntoIterator::into_iter(output_list);
//             }
//         }
//         return IntoIterator::into_iter(Vec::new());
//     }
//
//     fn has(events: &Events<E>) -> bool {
//         let channel_kind: ChannelKind = ChannelKind::of::<C>();
//         if let Some(channel_map) = events.messages.get(&channel_kind) {
//             let message_kind: MessageKind = MessageKind::of::<M>();
//             return channel_map.contains_key(&message_kind);
//         }
//         return false;
//     }
// }

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::tests::{
        Channel1, Channel2, Message1, Message2, MyMessageProtocol, MyProtocol,
    };

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
