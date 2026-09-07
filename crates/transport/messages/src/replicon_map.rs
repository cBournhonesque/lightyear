//! Shared entity-map resolution for (de)serialization.
//!
//! Local `Client` connections resolve entity mappings through
//! replicon's [`ServerEntityMap`](bevy_replicon::shared::server_entity_map::ServerEntityMap)
//! first, falling back to the local [`MessageManager`](crate::MessageManager) map for
//! lightyear-owned pairs (sender identities and similar) that replicon never sees.
//!
//! Server-side (`ClientOf`) connections always use the local map: replicon keeps no
//! server-side map, and the shared map in a host app describes the host's own
//! client-side view, not its remote peers.

use bevy_ecs::entity::Entity;
use bevy_ecs::entity::hash_map::EntityHashMap;
#[cfg(not(feature = "replicon"))]
use bevy_ecs::resource::Resource;
use bevy_ecs::system::{Res, SystemParam};

#[cfg(feature = "replicon")]
use bevy_replicon::shared::server_entity_map::ServerEntityMap;

impl RepliconMapParam<'_> {
    /// Shared send table (remote entities by local entity) for one connection.
    ///
    /// Used as the view's primary map, falling back to the connection-local map
    /// for lightyear-owned pairs (sender identities and similar) that replicon
    /// never sees. `None` unless the connection opts in *and* the `replicon`
    /// feature provides the shared map (absent on servers and without replication).
    pub(crate) fn shared_send_map(&self, use_shared: bool) -> Option<&EntityHashMap<Entity>> {
        #[cfg(feature = "replicon")]
        if use_shared && let Some(map) = self.map.as_deref() {
            return Some(map.to_server());
        }
        let _ = (self, use_shared);
        None
    }

    /// Shared receive table (local entities by remote entity) for one connection.
    ///
    /// Same policy as [`RepliconMapParam::shared_send_map`], other direction.
    pub(crate) fn shared_recv_map(&self, use_shared: bool) -> Option<&EntityHashMap<Entity>> {
        #[cfg(feature = "replicon")]
        if use_shared && let Some(map) = self.map.as_deref() {
            return Some(map.to_client());
        }
        let _ = (self, use_shared);
        None
    }
}

/// SystemParam exposing replicon's entity map when the `replicon` feature is enabled.
///
/// Present in built systems regardless of the feature so `MessagePlugin::finish`
/// needs no feature-branched tuples; always `None` when the feature is off.
#[cfg(feature = "replicon")]
#[derive(SystemParam)]
pub struct RepliconMapParam<'w> {
    /// Replicon's client-side entity map. Absent on servers and in apps without replication.
    pub map: Option<Res<'w, ServerEntityMap>>,
}

/// Feature-off stand-in: a valid [`SystemParam`](bevy_ecs::system::SystemParam) that
/// always resolves to `None`, so system signatures stay identical across features.
#[cfg(not(feature = "replicon"))]
#[derive(SystemParam)]
pub struct RepliconMapParam<'w> {
    pub map: Option<Res<'w, NeverMap>>,
}

/// Uninhabited marker: `Res<NeverMap>` can never exist, so the feature-off
/// [`RepliconMapParam`] always yields `None`.
#[cfg(not(feature = "replicon"))]
#[derive(Resource)]
pub enum NeverMap {}
