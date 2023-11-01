use std::any::TypeId;
use std::collections::HashMap;

use crate::channel::channel::{Channel, ChannelBuilder, ChannelSettings};
use crate::protocol::registry::{NetId, TypeKind, TypeMapper};
use crate::protocol::BitSerializable;
use crate::ChannelContainer;

/// ChannelKind - internal wrapper around the type of the channel
#[derive(Debug, Eq, Hash, Copy, Clone, PartialEq)]
pub struct ChannelKind(TypeId);

pub type ChannelId = NetId;

impl ChannelKind {
    pub fn of<C: Channel>() -> Self {
        Self(TypeId::of::<C>())
    }
}

impl TypeKind for ChannelKind {}

impl From<TypeId> for ChannelKind {
    fn from(type_id: TypeId) -> Self {
        Self(type_id)
    }
}

#[derive(Default, Clone)]
pub struct ChannelRegistry {
    // we only store the ChannelBuilder because we might want to create multiple instances of the same channel
    pub(in crate::protocol) builder_map: HashMap<ChannelKind, ChannelBuilder>,
    pub(in crate::protocol) kind_map: TypeMapper<ChannelKind>,
    built: bool,
}
impl ChannelRegistry {
    pub fn new() -> Self {
        Self {
            builder_map: HashMap::new(),
            kind_map: TypeMapper::new(),
            built: false,
        }
    }

    /// Build all the channels in the registry
    pub fn channels(&self) -> HashMap<ChannelKind, ChannelContainer> {
        let mut channels = HashMap::new();
        for (type_id, builder) in self.builder_map.iter() {
            channels.insert(*type_id, builder.build());
        }
        channels
    }

    pub fn kind_map(&self) -> TypeMapper<ChannelKind> {
        self.kind_map.clone()
    }

    /// Register a new type
    pub fn add<T: Channel>(&mut self, settings: ChannelSettings) {
        let kind = self.kind_map.add::<T>();
        self.builder_map.insert(kind, T::get_builder(settings));
    }

    /// get the registered object for a given type
    pub fn get_builder_from_kind(&self, channel_kind: &ChannelKind) -> Option<&ChannelBuilder> {
        self.builder_map.get(channel_kind)
    }

    pub fn get_kind_from_net_id(&self, channel_id: ChannelId) -> Option<&ChannelKind> {
        self.kind_map.kind(channel_id)
    }

    pub fn get_net_from_kind(&self, kind: &ChannelKind) -> Option<&NetId> {
        self.kind_map.net_id(kind)
    }

    pub fn get_builder_from_net_id(&self, channel_id: ChannelId) -> Option<&ChannelBuilder> {
        let channel_kind = self.get_kind_from_net_id(channel_id)?;
        self.get_builder_from_kind(channel_kind)
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.kind_map.len()
    }
}

#[cfg(test)]
mod tests {
    use lightyear_derive::ChannelInternal;

    use crate::{ChannelDirection, ChannelMode, ChannelSettings};

    use super::*;

    #[derive(ChannelInternal)]
    pub struct MyChannel;

    #[test]
    fn test_channel_registry() -> anyhow::Result<()> {
        let mut registry = ChannelRegistry::new();

        let settings = ChannelSettings {
            mode: ChannelMode::UnorderedUnreliable,
            direction: ChannelDirection::Bidirectional,
        };
        registry.add::<MyChannel>(settings.clone());
        assert_eq!(registry.len(), 1);

        let mut builder = registry.get_builder_from_net_id(0).unwrap();
        let channel_container: ChannelContainer = builder.build();
        assert_eq!(
            channel_container.setting.mode,
            ChannelMode::UnorderedUnreliable
        );
        Ok(())
    }
}
