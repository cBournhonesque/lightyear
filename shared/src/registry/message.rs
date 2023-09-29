use crate::packet::message::{Message, MessageBuilder, MessageContainer};
use crate::registry::NetId;
use crate::type_registry;
use anyhow::bail;
use bitcode::{Decode, Encode};
use std::any::TypeId;
use std::collections::HashMap;

/// MessageKind - internal wrapper around the type of the channel
#[derive(Eq, Hash, Copy, Clone, PartialEq)]
pub struct MessageKind(TypeId);

impl MessageKind {
    pub fn of<M: Message>() -> Self {
        Self(TypeId::of::<M>())
    }
}

pub struct MessageRegistry {
    pub(in crate::registry) next_net_id: NetId,
    pub(in crate::registry) kind_map: HashMap<MessageKind, (NetId, Box<dyn MessageBuilder>)>,
    pub(in crate::registry) id_map: HashMap<NetId, MessageKind>,
    built: bool,
}
impl MessageRegistry {
    pub fn new() -> Self {
        Self {
            next_net_id: 0,
            kind_map: HashMap::new(),
            id_map: HashMap::new(),
            built: false,
        }
    }

    /// Register a new type
    pub fn add<T: Message + 'static>(&mut self) -> anyhow::Result<()> {
        let channel_kind = MessageKind(TypeId::of::<T>());
        if self.kind_map.contains_key(&channel_kind) {
            bail!("Message type already registered");
        }
        let net_id = self.next_net_id;
        self.kind_map
            .insert(channel_kind, (net_id, T::get_builder()));
        self.id_map.insert(net_id, channel_kind);
        self.next_net_id += 1;
        Ok(())
    }

    /// Get the registered object for a given type
    pub fn get_builder_from_kind(
        &self,
        channel_kind: &MessageKind,
    ) -> Option<Box<dyn MessageBuilder>> {
        self.kind_map
            .get(channel_kind)
            .and_then(|(_, t)| Some((*t).clone()))
    }

    pub fn get_kind_from_net_id(&self, net_id: NetId) -> Option<&MessageKind> {
        self.id_map.get(&net_id).and_then(|k| Some(k))
    }

    pub fn get_net_from_kind(&self, kind: &MessageKind) -> Option<&NetId> {
        self.kind_map
            .get(&kind)
            .and_then(|(net_id, _)| Some(net_id))
    }

    pub fn get_from_net_id(&self, net_id: NetId) -> Option<Box<dyn MessageBuilder>> {
        let channel_kind = self.get_kind_from_id(net_id)?;
        self.get_from_type(channel_kind)
    }
    #[cfg(test)]
    fn len(&self) -> usize {
        self.kind_map.len()
    }
}
