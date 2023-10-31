use std::collections::HashMap;
use std::iter;

use crate::netcode::ClientId;
use bevy::prelude::{Component, Entity, Event};

use crate::protocol::message::MessageKind;
use crate::{Channel, ChannelKind, Message, MessageBehaviour, Protocol};

// TODO: don't make fields pub but instead make accessors
pub struct ConnectionEvents<P: Protocol> {
    // netcode
    pub connection: bool,
    pub disconnection: bool,

    // messages
    pub messages: HashMap<MessageKind, HashMap<ChannelKind, Vec<P::Message>>>,
    // replication
    pub spawns: Vec<Entity>,
    pub despawns: Vec<Entity>,
    // TODO: key by entity or by kind?
    pub insert_components: HashMap<Entity, Vec<P::Components>>,
    pub remove_components: HashMap<Entity, Vec<P::ComponentKinds>>,
    pub update_components: HashMap<Entity, Vec<P::Components>>,
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
            // messages
            messages: HashMap::new(),
            // replication
            spawns: Vec::new(),
            despawns: Vec::new(),
            insert_components: Default::default(),
            remove_components: Default::default(),
            update_components: Default::default(),
            // bookkeeping
            empty: true,
        }
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

    // TODO: add channel_kind in the output? add channel as a generic parameter?
    // pub fn into_iter_messages<M: Message>(&mut self) -> impl Iterator<Item = M>
    // where
    //     // M: From<P::Message>,
    //     // TODO: this Error = () bound is not ideal..
    //     P::Message: TryInto<M, Error = ()>,
    // {
    //     let message_kind = MessageKind::of::<M>();
    //     self.messages
    //         .remove(&message_kind)
    //         .into_iter()
    //         .flat_map(|data| {
    //             data.into_iter().flat_map(|(_, messages)| {
    //                 messages.into_iter().map(|message| {
    //                     // SAFETY: we checked via message kind that only messages of the type M
    //                     // are in the list
    //                     message.try_into().unwrap()
    //                 })
    //             })
    //         })
    // }

    // pub fn has_messages<M: Message>(&self) -> bool {
    //     let message_kind = MessageKind::of::<M>();
    //     self.messages.contains_key(&message_kind)
    // }

    // pub fn into_iter_messages_from_channel<M: Message, C: Channel>(
    //     &mut self,
    // ) -> impl Iterator<Item = M> {
    //     let message_kind = MessageKind::of::<M>();
    //     let channel_kind = ChannelKind::of::<C>();
    //     if let Some(data) = self.messages.remove(&message_kind) {
    //         if let Some(data) = data.remove(&channel_kind) {
    //             return data.into_iter();
    //         }
    //     }
    //     return Vec::new().into_iter();
    // }

    // pub fn into_iter_component<C: Component>(&mut self) -> impl Iterator<>

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

    pub(crate) fn push_insert_component(&mut self, entity: Entity, component: P::Components) {
        self.insert_components
            .entry(entity)
            .or_default()
            .push(component);
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

// pub trait IterComponentInsertEvent<P: Protocol, Ctx: EventContext = ()> {
//     fn into_iter_entity_spawn<C: Component>(
//         &mut self,
//     ) -> Box<dyn Iterator<Item = (Entity, Ctx)> + '_>;
//     fn has_entity_spawn<C: Component>(&self) -> bool;
// }
//
// impl<P: Protocol> IterComponentInsertEvent for ConnectionEvents<P> {
//     fn into_iter_entity_spawn(&mut self) -> Box<dyn Iterator<Item = (Entity, ())> + '_> {
//         let spawns = std::mem::take(&mut self.spawns);
//         Box::new(spawns.into_iter().map(|entity| (entity, ())))
//     }
//
//     fn has_entity_spawn(&self) -> bool {
//         !self.spawns.is_empty()
//     }
// }

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
