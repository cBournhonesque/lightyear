use bevy::ecs::entity::MapEntities;
use std::any::TypeId;
use std::fmt::{Debug, Display};
use std::hash::Hash;

use bevy::prelude::{App, Component, Entity, EntityMapper, EntityWorldMut, World};
use bevy::utils::HashMap;
use cfg_if::cfg_if;

use crate::_reexport::{InstantCorrector, NullInterpolator};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::client::components::{ComponentSyncMode, LerpFn, SyncMetadata};
use crate::prelude::{Message, Named, PreSpawnedPlayerObject};
use crate::protocol::{BitSerializable, EventContext, Protocol};
use crate::shared::events::connection::{
    IterComponentInsertEvent, IterComponentRemoveEvent, IterComponentUpdateEvent,
};
use crate::shared::replication::components::ShouldBePredicted;
use crate::shared::replication::components::{PrePredicted, ShouldBeInterpolated};
use crate::shared::replication::ReplicationSend;

// client writes an Enum containing all their message type
// each message must derive message

// that big enum will implement MessageProtocol via a proc macro
// TODO: remove the extra  Serialize + DeserializeOwned + Clone  bounds
pub trait ComponentProtocol:
    BitSerializable
    + Serialize
    + DeserializeOwned
    + MapEntities
    + ComponentBehaviour
    + Debug
    + Send
    + Sync
    + From<ShouldBePredicted>
    + From<PrePredicted>
    + From<ShouldBeInterpolated>
    + TryInto<ShouldBePredicted>
    + TryInto<PrePredicted>
{
    type Protocol: Protocol;

    /// Map from the type-id to the component kind for each component in the protocol
    fn type_ids() -> HashMap<TypeId, <Self::Protocol as Protocol>::ComponentKinds>;

    /// Apply a ComponentInsert to an entity
    fn insert(self, entity: &mut EntityWorldMut);

    /// Apply a ComponentUpdate to an entity
    fn update(self, entity: &mut EntityWorldMut);

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

    /// Add all component systems for the PrepareInterpolation SystemSet
    fn add_prepare_interpolation_systems(app: &mut App);

    /// Add all component systems for the Interpolation SystemSet
    fn add_interpolation_systems(app: &mut App);

    // /// Get the sync mode for the component
    // fn mode<C>() -> ComponentSyncMode
    // where
    //     Self: SyncMetadata<C>,
    // {
    //     <Self as SyncMetadata<C>>::mode()
    // }

    /// If false, we don't want to apply any interpolation
    fn has_interpolation<C>() -> bool
    where
        Self: SyncMetadata<C>,
    {
        TypeId::of::<<Self as SyncMetadata<C>>::Interpolator>() != TypeId::of::<NullInterpolator>()
    }

    /// If false, we don't want to apply any corrections
    fn has_correction<C>() -> bool
    where
        Self: SyncMetadata<C>,
    {
        TypeId::of::<<Self as SyncMetadata<C>>::Corrector>() != TypeId::of::<InstantCorrector>()
    }

    /// Interpolate the component between two states, using the Interpolator associated with the component
    fn lerp<C>(start: &C, other: &C, t: f32) -> C
    where
        Self: SyncMetadata<C>,
    {
        <Self as SyncMetadata<C>>::Interpolator::lerp(start, other, t)
    }

    /// Visually correct the component between two states, using the Corrector associated with the component
    fn correct<C>(predicted: &C, corrected: &C, t: f32) -> C
    where
        Self: SyncMetadata<C>,
    {
        <Self as SyncMetadata<C>>::Corrector::lerp(predicted, corrected, t)
    }
}

// TODO: enum_delegate doesn't work with generics + cannot be used multiple times since it derives a bunch of Into/From traits
/// Trait to delegate a method from the ComponentProtocol enum to the inner Component type
///  We use it mainly for the IntoKind, From implementations
#[enum_delegate::register]
pub trait ComponentBehaviour {}

impl<T: Component + Message> ComponentBehaviour for T {}

// Trait that lets us convert a component type into the corresponding ComponentProtocolKind
// #[cfg(feature = "leafwing")]
// pub trait FromTypes: FromType<ShouldBePredicted> + FromType<ShouldBeInterpolated> {}
//
// #[cfg(not(feature = "leafwing"))]
// pub trait FromTypes: FromType<ShouldBePredicted> + FromType<ShouldBeInterpolated> {}

cfg_if!(
    if #[cfg(feature = "leafwing")] {
        use leafwing_input_manager::prelude::ActionState;
        pub trait ComponentProtocolKind:
            BitSerializable
            + Serialize
            + DeserializeOwned
            + PartialEq
            + Eq
            + PartialOrd
            + Ord
            + Clone
            + Copy
            + Hash
            + Debug
            + Send
            + Sync
            + Display
            + for<'a> From<&'a <Self::Protocol as Protocol>::Components>
            + ComponentKindBehaviour
            + FromType<ShouldBePredicted>
            + FromType<ShouldBeInterpolated>
            + FromType<PrePredicted>
            + FromType<PreSpawnedPlayerObject>
            + FromType<ActionState<<Self::Protocol as Protocol>::LeafwingInput1>>
            + FromType<ActionState<<Self::Protocol as Protocol>::LeafwingInput2>>
        {
            type Protocol: Protocol;
        }
    } else {
        pub trait ComponentProtocolKind:
            BitSerializable
            + Serialize
            + DeserializeOwned
            + PartialEq
            + Eq
            + PartialOrd
            + Ord
            + Clone
            + Copy
            + Hash
            + Debug
            + Send
            + Sync
            + Display
            + for<'a> From<&'a <Self::Protocol as Protocol>::Components>
            + ComponentKindBehaviour
            + FromType<ShouldBePredicted>
            + FromType<ShouldBeInterpolated>
            + FromType<PrePredicted>
            + FromType<PreSpawnedPlayerObject>
        {
            type Protocol: Protocol;
        }
    }
);

/// Trait to delegate a method from the ComponentProtocolKind enum to the inner Component type
pub trait ComponentKindBehaviour {
    /// Remove the component for an entity
    fn remove(self, entity: &mut EntityWorldMut);
}

// /// Trait to convert a component type into the corresponding ComponentProtocolKind
// pub trait IntoKind<K: ComponentProtocolKind> {
//     fn into_kind() -> K;
// }

// TODO: prefer FromType to IntoKind because IntoKind requires adding an additional bound to the component type,
//  which is not possible for external components.
//  (e.g. impl IntoKind for ActionState both the trait and the type are external to the user's crate)
/// Trait to convert a component type into the corresponding ComponentProtocolKind
pub trait FromType<T> {
    fn from_type() -> Self;
}
