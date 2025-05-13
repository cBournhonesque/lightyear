use crate::client::{Connecting, PeerMetadata};
use bevy::ecs::component::HookContext;
use bevy::ecs::error::HandleError;
use bevy::ecs::error::{ignore, panic, CommandWithEntity};
use bevy::ecs::relationship::{Relationship, RelationshipHookMode, RelationshipSourceCollection};
use bevy::ecs::system::entity_command;
use bevy::ecs::world::DeferredWorld;
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use tracing::warn;

use crate::prelude::NetworkTarget;
#[cfg(not(feature = "std"))]
use alloc::{boxed::Box, vec, vec::Vec};
use lightyear_core::id::PeerId;
use lightyear_core::prelude::{LocalTimeline, NetworkTimeline};
use lightyear_link::prelude::LinkOf;
use smallvec::SmallVec;


/// Marker component to identify this link as a LinkOf
///
/// This is equivalent to LinkOf + Connected.
#[derive(Component, Clone, Copy, PartialEq, Eq, Debug, Reflect)]
#[reflect(Component, PartialEq, Debug, Clone)]
pub struct ClientOf;

