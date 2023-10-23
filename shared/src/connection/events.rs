use std::collections::HashMap;

use crate::netcode::ClientId;
use bevy_ecs::prelude::Entity;

use crate::{Channel, ChannelKind, Message, Protocol};

// TODO: don't make fields pub but instead make accessors
pub struct ConnectionEvents<P: Protocol> {
    // netcode
    // pub connections: Vec<ClientId>,
    // pub disconnections: Vec<ClientId>,

    // messages
    // TODO: add MessageKinds?
    pub messages: HashMap<ChannelKind, Vec<P::Message>>,
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
            // connections: Vec::new(),
            // disconnections: Vec::new(),
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

    pub fn is_empty(&self) -> bool {
        self.empty
    }
    pub(crate) fn push_message(&mut self, channel_kind: ChannelKind, message: P::Message) {
        self.messages.entry(channel_kind).or_default().push(message);
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

pub trait IterConnectionEvent<P: Protocol> {
    type Iter;
    type IntoIter;

    fn iter(events: &mut ConnectionEvents<P>) -> Self::Iter;

    fn has(events: &ConnectionEvents<P>) -> bool;
}

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
