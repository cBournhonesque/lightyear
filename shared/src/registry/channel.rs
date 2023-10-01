use std::any::TypeId;
use std::collections::HashMap;

use anyhow::bail;

use crate::channel::channel::{Channel, ChannelBuilder, ChannelSettings};
use crate::registry::NetId;
use crate::ChannelContainer;

/// ChannelKind - internal wrapper around the type of the channel
#[derive(Debug, Eq, Hash, Copy, Clone, PartialEq)]
pub struct ChannelKind(TypeId);

pub struct ChannelRegistry<P> {
    pub(in crate::registry) next_net_id: NetId,
    pub(in crate::registry) kind_map: HashMap<ChannelKind, (NetId, ChannelBuilder<P>)>,
    pub(in crate::registry) id_map: HashMap<NetId, ChannelKind>,
    built: bool,
}
impl<P> ChannelRegistry<P> {
    pub fn new() -> Self {
        Self {
            next_net_id: 0,
            kind_map: HashMap::new(),
            id_map: HashMap::new(),
            built: false,
        }
    }

    /// Build all the channels in the registry
    pub fn channels(&self) -> HashMap<ChannelKind, ChannelContainer<P>> {
        let mut channels = HashMap::new();
        for (type_id, (_, builder)) in self.kind_map.iter() {
            channels.insert(*type_id, builder.build());
        }
        channels
    }

    /// Register a new type
    pub fn add<T: Channel + 'static>(&mut self, settings: ChannelSettings) -> anyhow::Result<()> {
        let channel_kind = ChannelKind(TypeId::of::<T>());
        if self.kind_map.contains_key(&channel_kind) {
            bail!("Channel type already registered");
        }
        let net_id = self.next_net_id;
        self.kind_map
            .insert(channel_kind, (net_id, T::get_builder(settings)));
        self.id_map.insert(net_id, channel_kind);
        self.next_net_id += 1;
        Ok(())
    }

    /// get the registered object for a given type
    pub fn get_builder_from_kind(&self, channel_kind: &ChannelKind) -> Option<&ChannelBuilder<P>> {
        self.kind_map.get(channel_kind).and_then(|(_, t)| Some(t))
    }

    pub fn get_kind_from_net_id(&self, net_id: NetId) -> Option<&ChannelKind> {
        self.id_map.get(&net_id).and_then(|k| Some(k))
    }

    pub fn get_net_from_kind(&self, kind: &ChannelKind) -> Option<&NetId> {
        self.kind_map
            .get(&kind)
            .and_then(|(net_id, _)| Some(net_id))
    }

    pub fn get_builder_from_net_id(&self, net_id: NetId) -> Option<&ChannelBuilder<P>> {
        let channel_kind = self.get_kind_from_net_id(net_id)?;
        self.get_builder_from_kind(channel_kind)
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.kind_map.len()
    }
}

// impl ChannelRegistry {
//     fn build(&self) -> HashMap<NetId, ChannelContainer> {
//         let channels = self.id_map
//             .iter()
//             .map(|(net_id, type_id)| (*net_id, self.kind_map.get
//             .collect();
//     }
// }

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
        registry.add::<MyChannel>(settings.clone())?;
        assert_eq!(registry.len(), 1);

        let mut builder = registry.get_builder_from_net_id(0).unwrap();
        let channel_container = builder.build();
        assert_eq!(
            channel_container.setting.mode,
            ChannelMode::UnorderedUnreliable
        );
        Ok(())
    }
}
