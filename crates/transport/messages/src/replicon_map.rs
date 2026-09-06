//! Shared entity-map resolution for (de)serialization.
//!
//! Local `Client` connections resolve entity mappings through
//! replicon's [`ServerEntityMap`](bevy_replicon::shared::server_entity_map::ServerEntityMap)
//! first, falling back to the local [`MessageManager`](crate::MessageManager) map for
//! lightyear-owned pairs (connection entity, host identity) that replicon never sees.
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

/// Shared lookup tables borrowed from replicon's entity map for one system run.
///
/// Send/receive views check these first and fall back to the connection-local map
/// for lightyear-owned pairs (connection entity, host identity) that replicon
/// never sees.
#[derive(Clone, Copy)]
pub(crate) struct SharedMaps<'a> {
    /// Remote (server) entities by local (client) entity: send side.
    pub to_server: &'a EntityHashMap<Entity>,
    /// Local (client) entities by remote (server) entity: receive side.
    pub to_client: &'a EntityHashMap<Entity>,
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

/// Borrow replicon's shared lookup tables when the `replicon` feature is enabled.
///
/// Always `None` when the feature is off, so call sites need no feature branches.
/// Server-side connections ignore the result (replicon keeps no server-side map).
pub(crate) fn shared_maps<'a>(replicon: &'a RepliconMapParam<'_>) -> Option<SharedMaps<'a>> {
    #[cfg(feature = "replicon")]
    if let Some(map) = replicon.map.as_deref() {
        return Some(SharedMaps {
            to_server: map.to_server(),
            to_client: map.to_client(),
        });
    }
    let _ = replicon;
    None
}
