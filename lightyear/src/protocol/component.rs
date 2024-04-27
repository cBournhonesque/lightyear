use anyhow::Context;
use bevy::app::PreUpdate;
use bevy::ecs::entity::MapEntities;
use std::any::TypeId;
use std::fmt::{Debug, Display};
use std::hash::Hash;

use bevy::prelude::{
    App, Component, Entity, EntityMapper, EntityWorldMut, IntoSystemConfigs, Resource, TypePath,
    World,
};
use bevy::reflect::{FromReflect, GetTypeRegistration};
use bevy::utils::HashMap;
use cfg_if::cfg_if;

use crate::_reexport::{
    InstantCorrector, NullInterpolator, ReadBuffer, ReadWordBuffer, ServerMarker, WriteBuffer,
    WriteWordBuffer,
};
use bitcode::Encode;
use bitcode::__private::Fixed;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::client::components::{ComponentSyncMode, LerpFn, SyncMetadata};
use crate::prelude::{
    client, server, ChannelDirection, Message, MessageRegistry, PreSpawnedPlayerObject,
    RemoteEntityMap, ReplicateResource, Tick,
};
use crate::protocol::message::MessageType;
use crate::protocol::registry::{NetId, TypeKind, TypeMapper};
use crate::protocol::{BitSerializable, EventContext, Protocol};
use crate::serialize::RawData;
use crate::server::events::emit_replication_events;
use crate::server::networking::is_started;
use crate::shared::events::connection::{
    ConnectionEvents, IterComponentInsertEvent, IterComponentRemoveEvent, IterComponentUpdateEvent,
};
use crate::shared::events::systems::push_component_events;
use crate::shared::replication::components::ShouldBePredicted;
use crate::shared::replication::components::{PrePredicted, ShouldBeInterpolated};
use crate::shared::replication::entity_map::EntityMap;
use crate::shared::replication::systems::register_replicate_component_send;
use crate::shared::replication::ReplicationSend;
use crate::shared::sets::InternalMainSet;

pub type ComponentNetId = NetId;

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
    pub map_entities: Option<unsafe fn()>,
    pub write: RawWriteFn,
    pub remove: RawRemoveFn,
}

type SerializeFn<C> = fn(&C, writer: &mut WriteWordBuffer) -> anyhow::Result<()>;
type DeserializeFn<C> = fn(reader: &mut ReadWordBuffer) -> anyhow::Result<C>;
type MapEntitiesFn<C> = fn(&mut C, entity_map: &mut EntityMap);

type RawRemoveFn = fn(&ComponentRegistry, &mut EntityWorldMut);
type RawWriteFn = fn(
    &ComponentRegistry,
    &mut ReadWordBuffer,
    &mut EntityWorldMut,
    &mut EntityMap,
    &mut ConnectionEvents,
) -> anyhow::Result<()>;

pub struct ComponentFns<C> {
    pub serialize: SerializeFn<C>,
    pub deserialize: DeserializeFn<C>,
    pub map_entities: Option<MapEntitiesFn<C>>,
    pub write: RawWriteFn,
    pub remove: RawRemoveFn,
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
            map_entities: self.map_entities.map(|m| unsafe { std::mem::transmute(m) }),
            write: unsafe { std::mem::transmute(self.write) },
            remove: unsafe { std::mem::transmute(self.remove) },
        }
    }
}

impl ComponentRegistry {
    pub fn net_id<C: Component>(&self) -> ComponentNetId {
        self.kind_map
            .net_id(&ComponentKind::of::<C>())
            .copied()
            .expect(format!("Component {} is not registered", std::any::type_name::<C>()).as_str())
    }
    pub fn get_net_id<C: Component>(&self) -> Option<ComponentNetId> {
        self.kind_map.net_id(&ComponentKind::of::<C>()).copied()
    }

    pub(crate) fn add_component<C: Component + Message>(&mut self) {
        let message_kind = self.kind_map.add::<C>();
        let serialize: SerializeFn<C> = <C as BitSerializable>::encode;
        let deserialize: DeserializeFn<C> = <C as BitSerializable>::decode;
        let write: RawWriteFn = Self::write::<C>;
        let remove: RawRemoveFn = Self::remove::<C>;
        self.fns_map.insert(
            message_kind,
            ErasedComponentFns {
                type_id: TypeId::of::<C>(),
                type_name: std::any::type_name::<C>(),
                serialize: unsafe { std::mem::transmute(serialize) },
                deserialize: unsafe { std::mem::transmute(deserialize) },
                map_entities: None,
                write,
                remove,
            },
        );
    }

    pub(crate) fn add_component_mapped<C: Component + Message + MapEntities>(&mut self) {
        let message_kind = self.kind_map.add::<C>();
        let serialize: SerializeFn<C> = <C as BitSerializable>::encode;
        let deserialize: DeserializeFn<C> = <C as BitSerializable>::decode;
        let map_entities: MapEntitiesFn<C> = <C as MapEntities>::map_entities::<EntityMap>;
        let write: RawWriteFn = Self::write::<C>;
        let remove: RawRemoveFn = Self::remove::<C>;
        self.fns_map.insert(
            message_kind,
            ErasedComponentFns {
                type_id: TypeId::of::<C>(),
                type_name: std::any::type_name::<C>(),
                serialize: unsafe { std::mem::transmute(serialize) },
                deserialize: unsafe { std::mem::transmute(deserialize) },
                map_entities: Some(unsafe { std::mem::transmute(map_entities) }),
                write,
                remove: unsafe { std::mem::transmute(remove) },
            },
        );
    }

    pub(crate) fn serialize<C: Component>(
        &self,
        component: &C,
        writer: &mut WriteWordBuffer,
    ) -> anyhow::Result<()> {
        let kind = ComponentKind::of::<C>();
        let erased_fns = self
            .fns_map
            .get(&kind)
            .context("the component is not part of the protocol")?;
        let fns = unsafe { erased_fns.typed::<C>() };
        let net_id = self.kind_map.net_id(&kind).unwrap();
        <WriteWordBuffer as WriteBuffer>::encode::<NetId>(writer, net_id, Fixed)?;
        (fns.serialize)(component, writer)
    }

    fn internal_deserialize<C: Component>(
        &self,
        reader: &mut ReadWordBuffer,
        entity_map: &mut EntityMap,
    ) -> anyhow::Result<(NetId, C)> {
        let net_id = reader.decode::<ComponentNetId>(Fixed)?;
        let kind = self
            .kind_map
            .kind(net_id)
            .context("unknown component kind")?;
        let erased_fns = self
            .fns_map
            .get(kind)
            .context("the component is not part of the protocol")?;
        let fns = unsafe { erased_fns.typed::<C>() };
        let mut component = (fns.deserialize)(reader)?;
        if let Some(map_entities) = fns.map_entities {
            map_entities(&mut component, entity_map);
        }
        Ok((net_id, component))
    }

    pub(crate) fn deserialize<C: Component>(
        &self,
        reader: &mut ReadWordBuffer,
        entity_map: &mut EntityMap,
    ) -> anyhow::Result<C> {
        let (_, component) = self.internal_deserialize(reader, entity_map)?;
        Ok(component)
    }

    /// SAFETY: the ReadWordBuffer must contain bytes corresponding to the correct component type
    pub(crate) fn raw_write(
        &self,
        reader: &mut ReadWordBuffer,
        entity_world_mut: &mut EntityWorldMut,
        entity_map: &mut EntityMap,
        events: &mut ConnectionEvents,
    ) -> anyhow::Result<()> {
        let net_id = reader.decode::<ComponentNetId>(Fixed)?;
        let kind = self
            .kind_map
            .kind(net_id)
            .context("unknown component kind")?;
        let erased_fns = self
            .fns_map
            .get(kind)
            .context("the component is not part of the protocol")?;
        (erased_fns.write)(self, reader, entity_world_mut, entity_map, events)
    }

    pub(crate) fn write<C: Component>(
        &self,
        reader: &mut ReadWordBuffer,
        entity_world_mut: &mut EntityWorldMut,
        entity_map: &mut EntityMap,
        events: &mut ConnectionEvents,
    ) -> anyhow::Result<()> {
        let (net_id, component) = self.internal_deserialize::<C>(reader, entity_map)?;
        let entity = entity_world_mut.id();
        let tick = Tick(0);
        // TODO: do we need the tick information in the event?
        // TODO: should we send the event based on on the message type (Insert/Update) or based on whether the component was actually inserted?
        if let Some(mut c) = entity_world_mut.get_mut::<C>() {
            events.push_update_component(entity, net_id, tick);
            *c = component;
        } else {
            events.push_insert_component(entity, net_id, tick);
            entity_world_mut.insert(component);
        }
        Ok(())
    }

    pub(crate) fn raw_remove(&self, net_id: ComponentNetId, entity_world_mut: &mut EntityWorldMut) {
        let kind = self.kind_map.kind(net_id).expect("unknown component kind");
        let erased_fns = self
            .fns_map
            .get(kind)
            .expect("the component is not part of the protocol");
        (erased_fns.remove)(self, entity_world_mut);
    }

    pub(crate) fn remove<C: Component>(&self, entity_world_mut: &mut EntityWorldMut) {
        entity_world_mut.remove::<C>();
    }
}

/// Add a component to the list of components that can be sent
pub trait AppComponentExt {
    fn add_component<C: Component + Message>(&mut self, direction: ChannelDirection);

    fn add_component_mapped<C: Component + Message + MapEntities>(
        &mut self,
        direction: ChannelDirection,
    );

    fn add_resource<R: Resource + Message>(&mut self, direction: ChannelDirection);

    fn add_resource_mapped<R: Resource + Message + MapEntities>(
        &mut self,
        direction: ChannelDirection,
    );
}

fn register_component_send<C: Component>(app: &mut App, direction: ChannelDirection) {
    match direction {
        ChannelDirection::ClientToServer => {
            register_replicate_component_send::<C, client::ConnectionManager>(app);
            crate::server::events::emit_replication_events::<C>(app);
        }
        ChannelDirection::ServerToClient => {
            register_replicate_component_send::<C, server::ConnectionManager>(app);
            crate::client::events::emit_replication_events::<C>(app);
        }
        ChannelDirection::Bidirectional => {
            register_replicate_component_send::<C, client::ConnectionManager>(app);
            register_replicate_component_send::<C, server::ConnectionManager>(app);
        }
    }
}

fn register_resource_send<R: Resource + Message>(app: &mut App, direction: ChannelDirection) {
    match direction {
        ChannelDirection::ClientToServer => {
            crate::shared::replication::resources::send::add_resource_send_systems::<
                client::ConnectionManager,
                R,
            >(app);
            crate::shared::replication::resources::receive::add_resource_receive_systems::<
                server::ConnectionManager,
                R,
            >(app);
        }
        ChannelDirection::ServerToClient => {
            crate::shared::replication::resources::send::add_resource_send_systems::<
                server::ConnectionManager,
                R,
            >(app);
            crate::shared::replication::resources::receive::add_resource_receive_systems::<
                client::ConnectionManager,
                R,
            >(app);
        }
        ChannelDirection::Bidirectional => {
            register_resource_send::<R>(app, ChannelDirection::ClientToServer);
            register_resource_send::<R>(app, ChannelDirection::ServerToClient);
        }
    }
}

impl AppComponentExt for App {
    fn add_component<C: Component + Message>(&mut self, direction: ChannelDirection) {
        if let Some(mut registry) = self.world.get_resource_mut::<ComponentRegistry>() {
            registry.add_component::<C>();
        } else {
            todo!("create a protocol");
        }
        register_component_send::<C>(self, direction);
    }

    fn add_component_mapped<C: Component + Message + MapEntities>(
        &mut self,
        direction: ChannelDirection,
    ) {
        if let Some(mut registry) = self.world.get_resource_mut::<ComponentRegistry>() {
            registry.add_component_mapped::<C>();
        } else {
            todo!("create a protocol");
        }
        register_component_send::<C>(self, direction);
    }

    fn add_resource<R: Resource + Message>(&mut self, direction: ChannelDirection) {
        self.add_component::<ReplicateResource<R>>(direction);
        register_resource_send::<R>(self, direction)
    }

    fn add_resource_mapped<R: Resource + Message + MapEntities>(
        &mut self,
        direction: ChannelDirection,
    ) {
        self.add_component_mapped::<ReplicateResource<R>>(direction);
        register_resource_send::<R>(self, direction)
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
    fn add_per_component_replication_send_systems<R: ReplicationSend>(app: &mut App);

    /// Add systems needed to replicate resources to remote
    fn add_resource_send_systems<R: ReplicationSend>(app: &mut App);

    /// Add systems needed to receive resources from remote
    fn add_resource_receive_systems<R: ReplicationSend>(app: &mut App);

    /// Adds Component-related events to the app
    fn add_events<Ctx: EventContext>(app: &mut App);

    // TODO: make this a system that runs after io-receive/recv/read
    //  maybe a standalone EventsPlugin
    /// Takes messages that were written and writes ComponentEvents
    fn push_component_events<
        E: IterComponentInsertEvent<Ctx>
            + IterComponentRemoveEvent<Ctx>
            + IterComponentUpdateEvent<Ctx>,
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
