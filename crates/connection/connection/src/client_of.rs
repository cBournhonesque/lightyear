use bevy_ecs::{component::Component, reflect::ReflectComponent};
use bevy_reflect::Reflect;

/// Marker component to identify this link as a [`LinkOf`](lightyear_link::prelude::LinkOf)
///
/// This is equivalent to `LinkOf` + [`Connected`](crate::prelude::Connected).
///
#[derive(Component, Clone, Copy, PartialEq, Eq, Debug, Reflect)]
#[reflect(Component, PartialEq, Debug, Clone)]
pub struct ClientOf;

/// Marker component to identify a link that should skip netcode processing.
///
/// For example a Server could have both a ServerNetcode and a SteamServerIO.
/// In which we case we want to skip netcode for the links that come from steam, since they
/// already have a RemoteId.
///
// TODO: maybe we could also apply netcode for steam links? the RemoteId::Steam would be overridden
//  with a RemoteId::Netcode identifier, and the SteamId could simply be some extra metadata component?
#[derive(Component, Default)]
pub struct SkipNetcode;
