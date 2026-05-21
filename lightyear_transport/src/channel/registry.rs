use crate::channel::Channel;
use crate::channel::builder::ChannelSettings;
use bevy_app::App;
use bevy_ecs::resource::Resource;
use bevy_platform::collections::HashMap;
use bevy_reflect::TypePath;
use core::any::TypeId;
use lightyear_connection::direction::NetworkDirection;
use lightyear_core::network::NetId;
use lightyear_utils::registry::{RegistryHash, RegistryHasher, TypeKind, TypeMapper};

/// Type-based identifier for a registered channel.
///
/// `ChannelKind` wraps the [`TypeId`] of the marker type implementing [`Channel`]. It is useful for
/// erased APIs such as [`Transport::send_erased`](crate::transport::Transport::send_erased)
/// when code knows the channel dynamically rather than as a generic type.
#[derive(Debug, Eq, Hash, Copy, Clone, PartialEq)]
pub struct ChannelKind(pub TypeId);

/// Stable network identifier assigned to a registered channel.
pub type ChannelId = NetId;

impl ChannelKind {
    /// Returns the kind for channel marker type `C`.
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

/// Registry of channel marker types, settings, and stable network IDs.
///
/// `ChannelRegistry` is an app-level resource. Register every channel type before transport
/// entities are spawned so client/server direction observers can create the correct
/// [`Transport`](crate::transport::Transport) sender and receiver state.
///
/// ### Adding channels
///
/// You can add a new channel to the registry by calling the [`add_channel`](ChannelRegistry::add_channel) method.
///
/// ```rust
/// use lightyear_transport::prelude::*;
/// use bevy_app::App;
/// use bevy_utils::default;
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
#[derive(Resource, Default, Clone, Debug, TypePath)]
pub struct ChannelRegistry {
    settings_map: HashMap<ChannelKind, ChannelSettings>,
    kind_map: TypeMapper<ChannelKind>,
    hasher: RegistryHasher,
}

impl ChannelRegistry {
    /// Returns settings for a channel kind.
    pub fn settings(&self, kind: ChannelKind) -> Option<&ChannelSettings> {
        self.settings_map.get(&kind)
    }

    pub(crate) fn settings_from_net_id(&self, net_id: NetId) -> Option<&ChannelSettings> {
        let kind = self.kind_map.kind(net_id)?;
        self.settings_map.get(kind)
    }

    /// Returns a clone of the internal type/network-ID map.
    ///
    /// This is primarily useful when other registries need to compare or persist the channel
    /// mapping.
    pub fn kind_map(&self) -> TypeMapper<ChannelKind> {
        self.kind_map.clone()
    }

    /// Registers channel marker type `C` with `settings`.
    ///
    /// If `C` was already registered, the existing [`ChannelKind`] and [`ChannelId`] are returned
    /// and the existing settings are left unchanged.
    pub fn add_channel<C: Channel>(
        &mut self,
        settings: ChannelSettings,
    ) -> (ChannelKind, ChannelId) {
        let kind = ChannelKind::of::<C>();
        if let Some(net_id) = self.kind_map.net_id(&kind) {
            return (kind, *net_id);
        }
        self.hasher.hash::<C>();
        self.settings_map.insert(kind, settings);
        let kind = self.kind_map.add::<C>();
        let net_id = self.get_net_from_kind(&kind).unwrap();
        (kind, *net_id)
    }

    /// Returns the debug/type name for a registered network ID.
    ///
    /// Returns `"Unknown"` if `net_id` is not registered.
    pub fn get_name_from_net_id(&self, net_id: ChannelId) -> &'static str {
        self.kind_map
            .kind(net_id)
            .and_then(|f| self.kind_map.name(f))
            .unwrap_or("Unknown")
    }

    /// Returns the debug/type name for a registered channel kind.
    ///
    /// Returns `"Unknown"` if `kind` is not registered.
    pub fn get_name_from_kind(&self, kind: &ChannelKind) -> &'static str {
        self.kind_map.name(kind).unwrap_or("Unknown")
    }

    /// Returns the channel kind registered for `channel_id`.
    pub fn get_kind_from_net_id(&self, channel_id: ChannelId) -> Option<&ChannelKind> {
        self.kind_map.kind(channel_id)
    }

    /// Returns the stable network ID registered for `kind`.
    pub fn get_net_from_kind(&self, kind: &ChannelKind) -> Option<&ChannelId> {
        self.kind_map.net_id(kind)
    }

    /// Finalizes and returns the registry hash.
    ///
    /// The hash lets peers validate that they registered the same channel types/settings during
    /// handshake or compatibility checks.
    pub fn finish(&mut self) -> RegistryHash {
        self.hasher.finish()
    }
}

/// Fluent registration helper returned by [`AppChannelExt::add_channel`].
///
/// Use [`add_direction`](Self::add_direction) to install client/server observers that populate
/// [`Transport`](crate::transport::Transport) entities with senders and receivers for this
/// channel.
pub struct ChannelRegistration<'a, C> {
    /// App being configured.
    pub app: &'a mut App,
    _marker: core::marker::PhantomData<C>,
}

impl<'a, C: Channel> ChannelRegistration<'a, C> {
    #[cfg(feature = "test_utils")]
    pub fn new<'b: 'a>(app: &'b mut App) -> Self {
        Self {
            app,
            _marker: core::marker::PhantomData,
        }
    }

    /// Adds a network direction for this channel.
    ///
    /// The installed observers depend on the current crate features:
    /// - with `client`, client transport entities get sender/receiver state for the requested
    ///   direction;
    /// - with `server`, server-side client link entities get the complementary state.
    pub fn add_direction(&mut self, direction: NetworkDirection) -> &mut Self {
        #[cfg(feature = "client")]
        self.add_client_direction(direction);
        #[cfg(feature = "server")]
        self.add_server_direction(direction);
        self
    }
}

/// Extension trait for registering transport channels on a Bevy [`App`].
pub trait AppChannelExt {
    /// Registers channel marker type `C` with `settings`.
    ///
    /// The returned [`ChannelRegistration`] should normally be used to call
    /// [`add_direction`](ChannelRegistration::add_direction), otherwise transport entities will know
    /// the channel exists but will not automatically receive sender/receiver state.
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
            _marker: core::marker::PhantomData,
        }
    }
}
