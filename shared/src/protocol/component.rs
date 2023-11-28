use std::fmt::Debug;
use std::hash::Hash;

use bevy::prelude::{App, Component, EntityWorldMut, World};
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::connection::events::{
    IterComponentInsertEvent, IterComponentRemoveEvent, IterComponentUpdateEvent,
};
use crate::protocol::{BitSerializable, EventContext, Protocol};
use crate::shared::replication::interpolation::ShouldBeInterpolated;
use crate::shared::replication::prediction::ShouldBePredicted;
use crate::shared::replication::ReplicationSend;

// client writes an Enum containing all their message type
// each message must derive message

// that big enum will implement MessageProtocol via a proc macro
// TODO: remove the extra  Serialize + DeserializeOwned + Clone  bounds
pub trait ComponentProtocol:
    BitSerializable
    + Serialize
    + DeserializeOwned
    + ComponentBehaviour
    + Send
    + Sync
    + From<ShouldBePredicted>
    + From<ShouldBeInterpolated>
{
    type Protocol: Protocol;

    /// Add systems to send component inserts/removes/updates
    fn add_per_component_replication_send_systems<R: ReplicationSend<Self::Protocol>>(
        app: &mut App,
    );

    /// Adds Component-related events to the app
    fn add_events<Ctx: EventContext>(app: &mut App);

    // TODO: make this a system that runs after io-receive/recv/read
    //  maybe a standalone EventsPlugin
    /// Takes messages that were written and writes MessageEvents
    fn push_component_events<
        E: IterComponentInsertEvent<Self::Protocol, Ctx>
            + IterComponentRemoveEvent<Self::Protocol, Ctx>
            + IterComponentUpdateEvent<Self::Protocol, Ctx>,
        Ctx: EventContext,
    >(
        world: &mut World,
        events: &mut E,
    );

    fn add_prediction_systems(app: &mut App);
    fn add_interpolation_systems(app: &mut App);
}

/// Trait to delegate a method from the ComponentProtocol enum to the inner Component type
#[enum_delegate::register]
pub trait ComponentBehaviour {
    /// Insert the component for an entity
    fn insert(self, entity: &mut EntityWorldMut);
}

impl<T: Component> ComponentBehaviour for T {
    fn insert(self, entity: &mut EntityWorldMut) {
        // only insert if the entity didn't have the component
        // if entity.get::<T>().is_none() {
        entity.insert(self);
        // }
    }
}

pub trait ComponentProtocolKind:
    BitSerializable
    + Serialize
    + DeserializeOwned
    + PartialEq
    + Eq
    + Hash
    + Debug
    + Send
    + Sync
    + for<'a> From<&'a <Self::Protocol as Protocol>::Components>
    + ComponentKindBehaviour
{
    type Protocol: Protocol;
}

/// Trait to delegate a method from the ComponentProtocolKind enum to the inner Component type
pub trait ComponentKindBehaviour {
    /// Remove the component for an entity
    fn remove(self, entity: &mut EntityWorldMut);
}

/// Trait to convert a component type into the corresponding ComponentProtocolKind
pub trait IntoKind<K: ComponentProtocolKind> {
    fn into_kind() -> K;
}
