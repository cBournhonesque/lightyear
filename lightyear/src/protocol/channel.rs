use bevy::reflect::Reflect;
use serde::Deserialize;
use std::any::TypeId;
use std::collections::HashMap;

use crate::channel::builder::ChannelContainer;
use crate::channel::builder::{Channel, ChannelBuilder, ChannelSettings};
use crate::protocol::registry::{NetId, TypeKind, TypeMapper};

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

/// Registry to store metadata about the various [`Channel`]
#[derive(Default, Clone, Debug, PartialEq)]
pub struct ChannelRegistry {
    // we only store the ChannelBuilder because we might want to create multiple instances of the same channel
    pub(in crate::protocol) builder_map: HashMap<ChannelKind, ChannelBuilder>,
    pub(in crate::protocol) kind_map: TypeMapper<ChannelKind>,
    pub(in crate::protocol) name_map: HashMap<ChannelKind, String>,
    built: bool,
}

impl ChannelRegistry {
    pub fn new() -> Self {
        Self {
            builder_map: HashMap::new(),
            kind_map: TypeMapper::new(),
            name_map: HashMap::new(),
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
        let name = T::type_name();
        self.name_map.insert(kind, name.to_string());
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

    pub fn name(&self, kind: &ChannelKind) -> Option<&str> {
        self.name_map.get(kind).map(|s| s.as_str())
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
    use bevy::prelude::default;
    use lightyear_macros::ChannelInternal;

    use crate::channel::builder::{ChannelDirection, ChannelMode, ChannelSettings};

    use super::*;

    #[derive(ChannelInternal)]
    pub struct MyChannel;

    #[test]
    fn test_channel_registry() -> anyhow::Result<()> {
        let mut registry = ChannelRegistry::new();

        let settings = ChannelSettings {
            mode: ChannelMode::UnorderedUnreliable,
            ..default()
        };
        registry.add::<MyChannel>(settings.clone());
        assert_eq!(registry.len(), 1);

        let builder = registry.get_builder_from_net_id(0).unwrap();
        let channel_container: ChannelContainer = builder.build();
        assert_eq!(
            channel_container.setting.mode,
            ChannelMode::UnorderedUnreliable
        );
        Ok(())
    }
}
