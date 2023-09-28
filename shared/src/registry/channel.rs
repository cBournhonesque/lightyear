use crate::channel::channel::{Channel, ChannelBuilder, ChannelSettings};
use crate::registry::NetId;
use crate::{type_registry, ChannelContainer};
use anyhow::bail;
use std::any::TypeId;
use std::collections::HashMap;

/// ChannelKind - internal wrapper around the type of the channel
#[derive(Debug, Eq, Hash, Copy, Clone, PartialEq)]
pub struct ChannelKind(TypeId);

pub struct ChannelRegistry {
    pub(in crate::registry) next_net_id: NetId,
    pub(in crate::registry) kind_map: HashMap<ChannelKind, (NetId, ChannelBuilder)>,
    pub(in crate::registry) id_map: HashMap<NetId, ChannelKind>,
    built: bool,
}
impl ChannelRegistry {
    pub fn new() -> Self {
        Self {
            next_net_id: 0,
            kind_map: HashMap::new(),
            id_map: HashMap::new(),
            built: false,
        }
    }

    /// Build all the channels in the registry
    pub fn channels(&self) -> HashMap<ChannelKind, ChannelContainer> {
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

    /// Get the registered object for a given type
    pub fn get_from_type(&self, channel_kind: &ChannelKind) -> Option<ChannelBuilder> {
        self.kind_map
            .get(channel_kind)
            .and_then(|(_, t)| Some((*t).clone()))
    }

    pub fn get_kind_from_id(&self, net_id: NetId) -> Option<&ChannelKind> {
        self.id_map.get(&net_id).and_then(|k| Some(k))
    }
    pub fn get_from_net_id(&self, net_id: NetId) -> Option<ChannelBuilder> {
        let channel_kind = self.get_kind_from_id(net_id)?;
        self.get_from_type(channel_kind)
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
    use super::NetId;
    use super::*;
    use crate::{ChannelDirection, ChannelMode, ChannelSettings};
    use lightyear_derive::ChannelInternal;

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

        let mut builder = registry.get_mut_from_net_id(0).unwrap();
        let channel_container = builder.build();
        assert_eq!(
            channel_container.setting.mode,
            ChannelMode::UnorderedUnreliable
        );
        Ok(())
    }
}
