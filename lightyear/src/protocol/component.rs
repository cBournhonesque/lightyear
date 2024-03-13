use std::any::TypeId;
use std::fmt::{Debug, Display};
use std::hash::Hash;

use bevy::prelude::{App, Component, Entity, EntityWorldMut, World};
use bevy::utils::HashMap;
use cfg_if::cfg_if;

use crate::_reexport::{InstantCorrector, NullInterpolator};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::client::components::{ComponentSyncMode, LerpFn, SyncMetadata};
use crate::prelude::{LightyearMapEntities, Message, Named, PreSpawnedPlayerObject};
use crate::protocol::{BitSerializable, EventContext, Protocol};
use crate::shared::events::connection::{
    IterComponentInsertEvent, IterComponentRemoveEvent, IterComponentUpdateEvent,
};
use crate::shared::replication::components::ShouldBeInterpolated;
use crate::shared::replication::components::ShouldBePredicted;
use crate::shared::replication::ReplicationSend;

// client writes an Enum containing all their message type
// each message must derive message

// that big enum will implement MessageProtocol via a proc macro
// TODO: remove the extra  Serialize + DeserializeOwned + Clone  bounds
pub trait ComponentProtocol:
    BitSerializable
    + Serialize
    + DeserializeOwned
    + LightyearMapEntities
    + ComponentBehaviour
    + Debug
    + Send
    + Sync
    + From<ShouldBePredicted>
    + From<ShouldBeInterpolated>
    + TryInto<ShouldBePredicted>
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

    /// Get the sync mode for the component
    fn lerp<C>(start: &C, other: &C, t: f32) -> C
    where
        Self: SyncMetadata<C>,
    {
        <Self as SyncMetadata<C>>::Interpolator::lerp(start, other, t)
    }

    fn correct<C>(predicted: &C, corrected: &C, t: f32) -> C
    where
        Self: SyncMetadata<C>,
    {
        <Self as SyncMetadata<C>>::Corrector::lerp(predicted, corrected, t)
    }
}

// /// Helper trait to wrap a component to replicate so that you can circumvent the orphan rule
// /// and implement new traits for an existing component.
// ///
// /// When replicating, the inner component will be replicated
// #[derive(Component, Deserialize, Serialize, Clone, Debug, PartialEq)]
// pub struct Wrapper<T: Message>(pub T);
//
// impl<T: Message> Named for Wrapper<T> {
//     fn name(&self) -> &'static str {
//         self.0.name()
//     }
// }
//
// impl<'a, T: Message> MapEntities<'a> for Wrapper<T> {
//     fn map_entities(&mut self, entity_mapper: Box<dyn EntityMapper + 'a>) {
//         self.0.map_entities(entity_mapper)
//     }
//
//     fn entities(&self) -> EntityHashSet<Entity> {
//         self.0.entities()
//     }
// }
//
// impl<T: Message> Message for Wrapper<T> {}
//
// TODO: enum_delegate doesn't work with generics + cannot be used multiple times since it derives a bunch of Into/From traits
/// Trait to delegate a method from the ComponentProtocol enum to the inner Component type
///  We use it mainly for the IntoKind, From implementations
#[enum_delegate::register]
pub trait ComponentBehaviour {}

impl<T: Component + Message> ComponentBehaviour for T {
    // // Apply a ComponentInsert to an entity
    // fn insert(self, entity: &mut EntityWorldMut) {
    //     // only insert if the entity didn't have the component
    //     // because otherwise the insert could override an component-update that was received later?
    //
    //     // but this could cause some issues if we wanted the component to be updated from the insert
    //     // if entity.get::<T>().is_none() {
    //     entity.insert(self);
    //     // }
    // }
    //
    // // Apply a ComponentUpdate to an entity
    // fn update(self, entity: &mut EntityWorldMut) {
    //     if let Some(mut c) = entity.get_mut::<T>() {
    //         *c = self;
    //     }
    //     // match entity.get_mut::<T>() {
    //     //     Some(mut c) => *c = self,
    //     //     None => {
    //     //         entity.insert(self);
    //     //     }
    //     // }
    // }
}

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
            + LightyearMapEntities
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
            + LightyearMapEntities
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
