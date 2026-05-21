//! Marker components for server-side client links.
//!
//! A server entity can own many link entities through [`LinkOf`](lightyear_link::prelude::LinkOf).
//! `ClientOf` marks one of those child links as a connection to a remote client.

use bevy_ecs::{component::Component, reflect::ReflectComponent};
use bevy_reflect::Reflect;

/// Marker component identifying a [`LinkOf`](lightyear_link::prelude::LinkOf) entity as a connected
/// client of a server.
///
/// In practice this is used alongside `LinkOf` and [`Connected`](crate::prelude::Connected) on the
/// server-side entity that represents one remote client.
#[derive(Component, Clone, Copy, PartialEq, Eq, Debug, Reflect)]
#[reflect(Component, PartialEq, Debug, Clone)]
pub struct ClientOf;

/// Marker component to identify a link that should skip netcode processing.
///
/// For example, a server can run both netcode and Steam IO. Steam links already have a
/// [`RemoteId`](lightyear_core::id::RemoteId), so the netcode server should ignore them instead of
/// trying to authenticate and rewrite their peer identity.
// TODO: maybe we could also apply netcode for steam links? the RemoteId::Steam would be overridden
//  with a RemoteId::Netcode identifier, and the SteamId could simply be some extra metadata component?
#[derive(Component, Default)]
pub struct SkipNetcode;
