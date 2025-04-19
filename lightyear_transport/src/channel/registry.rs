use crate::channel::builder::{AuthorityChannel, ChannelSettings, InputChannel};
use crate::channel::Channel;
use bevy::app::App;
use bevy::ecs::component::ComponentId;
use bevy::platform::collections::HashMap;
use bevy::prelude::{Resource, TypePath};
use core::any::TypeId;
use core::time::Duration;
use lightyear_core::network::NetId;
use lightyear_utils::registry::{TypeKind, TypeMapper};

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
/// use lightyear_transport::prelude::*;
/// use bevy::prelude::*;
///
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
    settings_map: HashMap<ChannelKind, ChannelSettings>,
    kind_map: TypeMapper<ChannelKind>,
    built: bool,
}

impl ChannelRegistry {
    pub(crate) fn new() -> Self {
        let mut registry = Self {
            settings_map: HashMap::default(),
            kind_map: TypeMapper::new(),
            built: false,
        };
        // registry.add_channel::<EntityUpdatesChannel>();
        // registry.add_channel::<EntityActionsChannel>();
        // registry.add_channel::<PingChannel>();
        // registry.add_channel::<PongChannel>();
        // registry.add_channel::<InputChannel>();
        // registry.add_channel::<AuthorityChannel>();
        registry
    }

    pub(crate) fn settings<C: Channel>(&self) -> Option<&ChannelSettings> {
        let kind = ChannelKind::of::<C>();
        self.settings_map.get(&kind)
    }

    pub(crate) fn settings_from_net_id(&self, net_id: NetId) -> Option<&ChannelSettings> {
        let kind = self.kind_map.kind(net_id)?;
        self.settings_map.get(kind)
    }

    pub fn kind_map(&self) -> TypeMapper<ChannelKind> {
        self.kind_map.clone()
    }

    /// Register a new type
    pub fn add_channel<C: Channel>(&mut self, settings: ChannelSettings) -> (ChannelKind, ChannelId) {
        let kind = ChannelKind::of::<C>();
        if let Some(net_id) = self.kind_map.net_id(&kind) {
            return (kind, *net_id);
        }
        self.settings_map.insert(kind, settings);
        let kind = self.kind_map.add::<C>();
        let net_id = self.get_net_from_kind(&kind).unwrap();
        (kind, *net_id)
    }

    pub fn get_kind_from_net_id(&self, channel_id: ChannelId) -> Option<&ChannelKind> {
        self.kind_map.kind(channel_id)
    }

    pub fn get_net_from_kind(&self, kind: &ChannelKind) -> Option<&ChannelId> {
        self.kind_map.net_id(kind)
    }
}

pub struct ChannelRegistration<'a, C> {
    pub app: &'a mut App,
    _marker: core::marker::PhantomData<C>
}


impl<C> ChannelRegistration<'_, C> {
    #[cfg(feature = "test_utils")]
    pub fn new(app: &mut App) -> Self {
        Self {
            app,
            _marker: core::marker::PhantomData,
        }
    }
}


/// Add a message to the list of messages that can be sent
pub trait AppChannelExt {
    fn add_channel<C: Channel>(&mut self, settings: ChannelSettings) -> ChannelRegistration<'_, C>;
}

impl AppChannelExt for App {
    fn add_channel<C: Channel>(&mut self, settings: ChannelSettings) -> ChannelRegistration<'_, C> {
        if !self.world().contains_resource::<ChannelRegistry>() {
            self.world_mut().init_resource::<ChannelRegistry>();
        }
        let mut registry = self.world_mut().resource_mut::<ChannelRegistry>();
        registry.add_channel::<C>(settings);
        ChannelRegistration {
            app: self,
            _marker: core::marker::PhantomData
        }
    }
}
