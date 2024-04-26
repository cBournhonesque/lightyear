use anyhow::Context;
use bevy::ecs::entity::MapEntities;
use std::any::TypeId;
use std::fmt::{Debug, Display};
use std::hash::Hash;

use bevy::prelude::{
    App, Component, Entity, EntityMapper, EntityWorldMut, Resource, TypePath, World,
};
use bevy::reflect::{FromReflect, GetTypeRegistration};
use bevy::utils::HashMap;
use cfg_if::cfg_if;

use crate::_reexport::{
    InstantCorrector, NullInterpolator, ReadBuffer, ReadWordBuffer, WriteWordBuffer,
};
use bitcode::__private::Fixed;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::client::components::{ComponentSyncMode, LerpFn, SyncMetadata};
use crate::prelude::{Message, PreSpawnedPlayerObject};
use crate::protocol::registry::{NetId, TypeKind, TypeMapper};
use crate::protocol::{BitSerializable, EventContext, Protocol};
use crate::shared::events::connection::{
    IterComponentInsertEvent, IterComponentRemoveEvent, IterComponentUpdateEvent,
};
use crate::shared::replication::components::ShouldBePredicted;
use crate::shared::replication::components::{PrePredicted, ShouldBeInterpolated};
use crate::shared::replication::ReplicationSend;

#[derive(Debug, Default, Clone, Resource, PartialEq, TypePath)]
pub struct ComponentRegistry {
    // TODO: maybe instead of ComponentFns, use an erased trait objects? like dyn ErasedSerialize + ErasedDeserialize ?
    //  but how do we deal with implementing behaviour for types that don't have those traits?
    fns_map: HashMap<ComponentKind, ErasedComponentFns>,
    pub(crate) kind_map: TypeMapper<ComponentKind>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ErasedComponentFns {
    type_id: TypeId,
    type_name: &'static str,

    // TODO: maybe use `Vec<MaybeUninit<u8>>` instead of unsafe fn(), like bevy?
    pub serialize: unsafe fn(),
    pub deserialize: unsafe fn(),
    // pub map_entities: Option<unsafe fn()>,
    // pub component_type: crate::protocol::message::ComponentType,
}

type SerializeFn<C> = fn(&C, writer: &mut WriteWordBuffer) -> anyhow::Result<()>;
type DeserializeFn<C> = fn(reader: &mut ReadWordBuffer) -> anyhow::Result<C>;

pub struct ComponentFns<C> {
    pub serialize: SerializeFn<C>,
    pub deserialize: DeserializeFn<C>,
    // TODO: how to handle map entities, since map_entities takes a generic arg?
    // pub map_entities: Option<fn<M: EntityMapper>(&mut self, entity_mapper: &mut M);>,
    // pub message_type: crate::protocol::message::ComponentType,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum ComponentType {
    /// This is a message for a [`LeafwingUserAction`](crate::inputs::leafwing::LeafwingUserAction)
    #[cfg(feature = "leafwing")]
    LeafwingInput,
    /// This is a message for a [`UserAction`](crate::inputs::native::UserAction)
    NativeInput,
    /// This is not an input message, but a regular [`Component`]
    Normal,
}

impl ErasedComponentFns {
    unsafe fn typed<C: Component>(&self) -> ComponentFns<C> {
        debug_assert_eq!(
            self.type_id,
            TypeId::of::<C>(),
            "The erased message fns were created for type {}, but we are trying to convert to type {}",
            self.type_name,
            std::any::type_name::<C>(),
        );

        ComponentFns {
            serialize: unsafe { std::mem::transmute(self.serialize) },
            deserialize: unsafe { std::mem::transmute(self.deserialize) },
            // message_type: self.message_type,
        }
    }
}

impl ComponentRegistry {
    // pub(crate) fn component_type(&self, net_id: NetId) -> ComponentType {
    //     let kind = self.kind_map.kind(net_id).unwrap();
    //     self.fns_map
    //         .get(kind)
    //         .map(|fns| fns.message_type)
    //         .unwrap_or(ComponentType::Normal)
    // }
    pub(crate) fn add_component<C: Component>(&mut self) {
        let message_kind = self.kind_map.add::<C>();
        let serialize: SerializeFn<C> = <C as BitSerializable>::encode;
        let deserialize: DeserializeFn<C> = <C as BitSerializable>::decode;
        self.fns_map.insert(
            message_kind,
            ErasedComponentFns {
                type_id: TypeId::of::<C>(),
                type_name: std::any::type_name::<C>(),
                serialize: unsafe { std::mem::transmute(serialize) },
                deserialize: unsafe { std::mem::transmute(deserialize) },
                // map_entities: None,
                // message_type,
            },
        );
    }

    pub(crate) fn serialize<C: Component>(
        &self,
        message: &C,
        writer: &mut WriteWordBuffer,
    ) -> anyhow::Result<()> {
        let kind = ComponentKind::of::<C>();
        let erased_fns = self
            .fns_map
            .get(&kind)
            .context("the message is not part of the protocol")?;
        let fns = unsafe { erased_fns.typed::<C>() };
        let net_id = self.kind_map.net_id(&kind).unwrap();
        writer.encode(net_id, Fixed)?;
        (fns.serialize)(message, writer)
    }

    pub(crate) fn deserialize<C: Component>(
        &self,
        reader: &mut ReadWordBuffer,
    ) -> anyhow::Result<C> {
        let net_id = reader.decode::<NetId>(Fixed)?;
        let kind = self.kind_map.kind(net_id).context("unknown message kind")?;
        let erased_fns = self
            .fns_map
            .get(kind)
            .context("the message is not part of the protocol")?;
        let fns = unsafe { erased_fns.typed::<C>() };
        (fns.deserialize)(reader)
    }
}

// that big enum will implement ComponentProtocol via a proc macro
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

    /// Add systems needed to replicate resources to remote
    fn add_resource_send_systems<R: ReplicationSend<Self::Protocol>>(app: &mut App);

    /// Add systems needed to receive resources from remote
    fn add_resource_receive_systems<R: ReplicationSend<Self::Protocol>>(app: &mut App);

    /// Adds Component-related events to the app
    fn add_events<Ctx: EventContext>(app: &mut App);

    // TODO: make this a system that runs after io-receive/recv/read
    //  maybe a standalone EventsPlugin
    /// Takes messages that were written and writes ComponentEvents
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

impl<C: Component + Message> ComponentBehaviour for C {}

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
            + FromReflect
            + TypePath
            + GetTypeRegistration
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
            + FromReflect
            + TypePath
            + GetTypeRegistration
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

/// [`ComponentKind`] is an internal wrapper around the type of the component
#[derive(Debug, Eq, Hash, Copy, Clone, PartialEq)]
pub struct ComponentKind(TypeId);

impl ComponentKind {
    pub fn of<C: Component>() -> Self {
        Self(TypeId::of::<C>())
    }
}

impl TypeKind for ComponentKind {}

impl From<TypeId> for ComponentKind {
    fn from(type_id: TypeId) -> Self {
        Self(type_id)
    }
}
