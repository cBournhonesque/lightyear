use bevy::app::App;
use bevy::prelude::{Resource, TypePath};
use bevy::utils::Duration;
use std::any::TypeId;
use std::collections::HashMap;

use crate::channel::builder::{Channel, ChannelBuilder, ChannelSettings, PongChannel};
use crate::channel::builder::{
    ChannelContainer, EntityActionsChannel, EntityUpdatesChannel, InputChannel, PingChannel,
};
use crate::prelude::{ChannelMode, ReliableSettings};
use crate::protocol::registry::{NetId, TypeKind, TypeMapper};

// TODO: derive Reflect once we reach bevy 0.14
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

/// Registry to store metadata about the various [`Channels`](Channel) to use to send messages.
///
/// ### Adding channels
///
/// You can add a new channel to the registry by calling the [`add_channel`](ChannelRegistry::add_channel) method.
///
/// ```rust
/// use lightyear::prelude::*;
/// use bevy::prelude::*;
///
/// #[derive(Channel)]
/// struct MyChannel;
///
/// # fn main() {
/// #  let mut app = App::new();
/// #  app.init_resource::<ChannelRegistry>();
///    app.add_channel::<MyChannel>(ChannelSettings {
///      mode: ChannelMode::UnorderedUnreliable,
///      ..default()
///    });
/// # }
/// ```
///
///
#[derive(Resource, Default, Clone, Debug, PartialEq, TypePath)]
pub struct ChannelRegistry {
    // we only store the ChannelBuilder because we might want to create multiple instances of the same channel
    pub(in crate::protocol) builder_map: HashMap<ChannelKind, ChannelBuilder>,
    pub(in crate::protocol) kind_map: TypeMapper<ChannelKind>,
    pub(in crate::protocol) name_map: HashMap<ChannelKind, String>,
    built: bool,
}

impl ChannelRegistry {
    pub(crate) fn new(input_send_interval: Duration) -> Self {
        let mut registry = Self {
            builder_map: HashMap::new(),
            kind_map: TypeMapper::new(),
            name_map: HashMap::new(),
            built: false,
        };
        registry.add_channel::<EntityUpdatesChannel>(ChannelSettings {
            mode: ChannelMode::UnorderedUnreliableWithAcks,
            // we do not send the send_frequency to `replication_interval` here
            // because we want to make sure that the entity updates for tick T
            // are sent on tick T, so we will set the `replication_interval`
            // directly on the replication_sender
            send_frequency: Duration::default(),
            priority: 1.0,
        });
        registry.add_channel::<EntityActionsChannel>(ChannelSettings {
            mode: ChannelMode::UnorderedReliable(ReliableSettings::default()),
            // we do not send the send_frequency to `replication_interval` here
            // because we want to make sure that the entity updates for tick T
            // are sent on tick T, so we will set the `replication_interval`
            // directly on the replication_sender
            send_frequency: Duration::default(),
            // we want to send the entity actions as soon as possible
            priority: 10.0,
        });
        registry.add_channel::<PingChannel>(ChannelSettings {
            mode: ChannelMode::SequencedUnreliable,
            send_frequency: Duration::default(),
            // we always want to include the ping in the packet
            priority: f32::INFINITY,
        });
        registry.add_channel::<PongChannel>(ChannelSettings {
            mode: ChannelMode::SequencedUnreliable,
            send_frequency: Duration::default(),
            // we always want to include the pong in the packet
            priority: f32::INFINITY,
        });
        registry.add_channel::<InputChannel>(ChannelSettings {
            mode: ChannelMode::UnorderedUnreliable,
            send_frequency: input_send_interval,
            // we always want to include the inputs in the packet
            priority: f32::INFINITY,
        });
        registry
    }

    /// Returns true if the net_id corresponds to a channel that is used for replication
    pub(crate) fn is_replication_channel(&self, net_id: NetId) -> bool {
        self.kind_map.kind(net_id).map_or(false, |kind| {
            *kind == ChannelKind::of::<EntityUpdatesChannel>()
                || *kind == ChannelKind::of::<EntityActionsChannel>()
        })
    }

    /// Returns true if the net_id corresponds to a channel that is used for replicating updates
    pub(crate) fn is_replication_update_channel(&self, net_id: NetId) -> bool {
        self.kind_map.kind(net_id).map_or(false, |kind| {
            *kind == ChannelKind::of::<EntityUpdatesChannel>()
        })
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
    pub fn add_channel<C: Channel>(&mut self, settings: ChannelSettings) {
        let kind = self.kind_map.add::<C>();
        self.builder_map.insert(kind, C::get_builder(settings));
        let name = C::name();
        self.name_map.insert(kind, name.to_string());
    }

    /// get the registered object for a given type
    pub fn get_builder_from_kind(&self, channel_kind: &ChannelKind) -> Option<&ChannelBuilder> {
        self.builder_map.get(channel_kind)
    }

    pub fn get_kind_from_net_id(&self, channel_id: ChannelId) -> Option<&ChannelKind> {
        self.kind_map.kind(channel_id)
    }

    pub fn get_net_from_kind(&self, kind: &ChannelKind) -> Option<&ChannelId> {
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

/// Add a message to the list of messages that can be sent
pub trait AppChannelExt {
    fn add_channel<C: Channel>(&mut self, settings: ChannelSettings);
}

impl AppChannelExt for App {
    fn add_channel<C: Channel>(&mut self, settings: ChannelSettings) {
        let mut registry = self.world_mut().resource_mut::<ChannelRegistry>();
        registry.add_channel::<C>(settings);
    }
}

#[cfg(test)]
mod tests {
    use bevy::prelude::{default, TypePath};
    use lightyear_macros::ChannelInternal;

    use crate::channel::builder::{ChannelMode, ChannelSettings};

    use super::*;

    #[derive(ChannelInternal, TypePath)]
    pub struct MyChannel;

    #[test]
    fn test_channel_registry() {
        let mut registry = ChannelRegistry::default();

        let settings = ChannelSettings {
            mode: ChannelMode::UnorderedUnreliable,
            ..default()
        };
        registry.add_channel::<MyChannel>(settings.clone());
        assert_eq!(registry.len(), 1);

        let builder = registry.get_builder_from_net_id(0).unwrap();
        let channel_container: ChannelContainer = builder.build();
        assert_eq!(
            channel_container.setting.mode,
            ChannelMode::UnorderedUnreliable
        );
    }
}
