use anyhow::Context;
use bevy::app::PreUpdate;
use bevy::ecs::entity::MapEntities;
use std::any::TypeId;
use std::fmt::{Debug, Display};
use std::hash::Hash;
use std::ops::{Add, Mul};

use bevy::prelude::{
    App, Component, Entity, EntityMapper, EntityWorldMut, IntoSystemConfigs, Resource, TypePath,
    World,
};
use bevy::reflect::{FromReflect, GetTypeRegistration};
use bevy::utils::HashMap;
use cfg_if::cfg_if;

use crate::_internal::{
    add_interpolation_systems, add_prediction_systems, add_prepare_interpolation_systems,
    InstantCorrector, LinearInterpolator, MessageKind, NullInterpolator, ReadBuffer,
    ReadWordBuffer, ServerMarker, WriteBuffer, WriteWordBuffer,
};
use bitcode::Encode;
use bitcode::__private::Fixed;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use tracing::{error, trace};

use crate::client::components::{ComponentSyncMode, SyncMetadata};
use crate::client::config::ClientConfig;
use crate::prelude::client::SyncComponent;
use crate::prelude::server::{ServerConfig, ServerPlugin};
use crate::prelude::{
    client, server, ChannelDirection, Message, MessageRegistry, PreSpawnedPlayerObject,
    RemoteEntityMap, ReplicateResource, Tick,
};
use crate::protocol::message::MessageType;
use crate::protocol::registry::{NetId, TypeKind, TypeMapper};
use crate::protocol::{BitSerializable, EventContext};
use crate::serialize::RawData;
use crate::server::networking::is_started;
use crate::shared::events::connection::{
    ConnectionEvents, IterComponentInsertEvent, IterComponentRemoveEvent, IterComponentUpdateEvent,
};
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
    pub prediction_mode: ComponentSyncMode,
    pub interpolation_mode: ComponentSyncMode,
    pub interpolation: Option<unsafe fn()>,
    pub correction: Option<unsafe fn()>,
}

type SerializeFn<C> = fn(&C, writer: &mut WriteWordBuffer) -> anyhow::Result<()>;
type DeserializeFn<C> = fn(reader: &mut ReadWordBuffer) -> anyhow::Result<C>;
type MapEntitiesFn<C> = fn(&mut C, entity_map: &mut EntityMap);

type RawRemoveFn = fn(&ComponentRegistry, &mut EntityWorldMut);
type RawWriteFn = fn(
    &ComponentRegistry,
    &mut ReadWordBuffer,
    ComponentNetId,
    &mut EntityWorldMut,
    &mut EntityMap,
    &mut ConnectionEvents,
) -> anyhow::Result<()>;

type LerpFn<C> = fn(start: &C, other: &C, t: f32) -> C;

pub trait Linear {
    fn lerp(start: &Self, other: &Self, t: f32) -> Self;
}

impl<C> Linear for C
where
    for<'a> &'a C: Mul<f32, Output = C>,
    C: Add<C, Output = C>,
{
    fn lerp(start: &Self, other: &Self, t: f32) -> Self {
        start * (1.0 - t) + other * t
    }
}

pub struct ComponentFns<C> {
    pub serialize: SerializeFn<C>,
    pub deserialize: DeserializeFn<C>,
    pub map_entities: Option<MapEntitiesFn<C>>,
    pub write: RawWriteFn,
    pub remove: RawRemoveFn,
    pub prediction_mode: ComponentSyncMode,
    pub interpolation_mode: ComponentSyncMode,
    pub interpolation: Option<LerpFn<C>>,
    pub correction: Option<LerpFn<C>>,
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
            prediction_mode: self.prediction_mode,
            interpolation_mode: self.interpolation_mode,
            interpolation: self
                .interpolation
                .map(|m| unsafe { std::mem::transmute(m) }),
            correction: self.correction.map(|m| unsafe { std::mem::transmute(m) }),
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

    pub(crate) fn register_component<C: Component + Message>(&mut self) {
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
                prediction_mode: ComponentSyncMode::default(),
                interpolation_mode: ComponentSyncMode::default(),
                interpolation: None,
                correction: None,
            },
        );
    }

    pub(crate) fn register_resource<R: Resource + Message>(&mut self) {
        self.register_component::<ReplicateResource<R>>();
    }

    // TODO: add map_entities for resources
    pub(crate) fn add_map_entities<C: MapEntities + 'static>(&mut self) {
        let kind = ComponentKind::of::<C>();
        let map_entities: MapEntitiesFn<C> = <C as MapEntities>::map_entities::<EntityMap>;
        let erased_fns = self
            .fns_map
            .get_mut(&kind)
            .expect("the component is not part of the protocol");
        erased_fns.map_entities = Some(unsafe { std::mem::transmute(map_entities) });
    }

    pub(crate) fn set_prediction_mode<C: Component>(&mut self, mode: ComponentSyncMode) {
        let kind = ComponentKind::of::<C>();
        let erased_fns = self
            .fns_map
            .get_mut(&kind)
            .expect("the component is not part of the protocol");
        erased_fns.prediction_mode = mode;
    }

    pub(crate) fn set_linear_correction<C: Component + Linear>(&mut self) {
        let correction_fn: LerpFn<C> = <C as Linear>::lerp;
        // let correction_fn: LerpFn<C> =
        //     |predicted, corrected, t| predicted * (1.0 - t) + corrected * t;
        let kind = ComponentKind::of::<C>();
        let erased_fns = self
            .fns_map
            .get_mut(&kind)
            .expect("the component is not part of the protocol");
        erased_fns.correction = Some(unsafe { std::mem::transmute(correction_fn) });
    }

    pub(crate) fn set_correction<C: Component>(&mut self, correction_fn: LerpFn<C>) {
        let kind = ComponentKind::of::<C>();
        let erased_fns = self
            .fns_map
            .get_mut(&kind)
            .expect("the component is not part of the protocol");
        erased_fns.correction = Some(unsafe { std::mem::transmute(correction_fn) });
    }

    pub(crate) fn set_interpolation_mode<C: Component>(&mut self, mode: ComponentSyncMode) {
        let kind = ComponentKind::of::<C>();
        let erased_fns = self
            .fns_map
            .get_mut(&kind)
            .expect("the component is not part of the protocol");
        erased_fns.interpolation_mode = mode;
    }

    pub(crate) fn set_linear_interpolation<C: Component + Linear>(&mut self) {
        let interpolation_fn: LerpFn<C> = <C as Linear>::lerp;
        // let interpolation_fn: LerpFn<C> = |start, end, t| start * (1.0 - t) + end * t;
        let kind = ComponentKind::of::<C>();
        let erased_fns = self
            .fns_map
            .get_mut(&kind)
            .expect("the component is not part of the protocol");
        erased_fns.interpolation = Some(unsafe { std::mem::transmute(interpolation_fn) });
    }

    pub(crate) fn set_interpolation<C: Component>(&mut self, interpolation_fn: LerpFn<C>) {
        let kind = ComponentKind::of::<C>();
        let erased_fns = self
            .fns_map
            .get_mut(&kind)
            .expect("the component is not part of the protocol");
        erased_fns.correction = Some(unsafe { std::mem::transmute(interpolation_fn) });
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
        trace!(
            ?net_id,
            "serializing component: {:?}",
            std::any::type_name::<C>()
        );
        <WriteWordBuffer as WriteBuffer>::encode::<NetId>(writer, net_id, Fixed)?;
        (fns.serialize)(component, writer)
    }

    /// Deserialize only the component value (the ComponentNetId has already been read)
    fn raw_deserialize<C: Component>(
        &self,
        reader: &mut ReadWordBuffer,
        net_id: ComponentNetId,
        entity_map: &mut EntityMap,
    ) -> anyhow::Result<C> {
        // let net_id = reader.decode::<ComponentNetId>(Fixed)?;
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
        Ok(component)
    }

    pub(crate) fn deserialize<C: Component>(
        &self,
        reader: &mut ReadWordBuffer,
        entity_map: &mut EntityMap,
    ) -> anyhow::Result<C> {
        let net_id = reader.decode::<ComponentNetId>(Fixed)?;
        self.raw_deserialize(reader, net_id, entity_map)
    }

    pub(crate) fn map_entities<C: Component>(&self, component: &mut C, entity_map: &mut EntityMap) {
        let kind = ComponentKind::of::<C>();
        let erased_fns = self
            .fns_map
            .get(&kind)
            .context("the component is not part of the protocol")
            .unwrap();
        let fns = unsafe { erased_fns.typed::<C>() };
        if let Some(map_entities) = fns.map_entities {
            map_entities(component, entity_map);
        }
    }

    pub(crate) fn prediction_mode<C: Component>(&self) -> ComponentSyncMode {
        let kind = ComponentKind::of::<C>();
        let erased_fns = self
            .fns_map
            .get(&kind)
            .context("the component is not part of the protocol")
            .unwrap();
        erased_fns.prediction_mode
    }

    pub(crate) fn interpolation_mode<C: Component>(&self) -> ComponentSyncMode {
        let kind = ComponentKind::of::<C>();
        let erased_fns = self
            .fns_map
            .get(&kind)
            .context("the component is not part of the protocol")
            .unwrap();
        erased_fns.interpolation_mode
    }

    pub(crate) fn has_correction<C: Component>(&self) -> bool {
        let kind = ComponentKind::of::<C>();
        let erased_fns = self
            .fns_map
            .get(&kind)
            .context("the component is not part of the protocol")
            .unwrap();
        erased_fns.correction.is_some()
    }
    pub(crate) fn correct<C: Component>(&self, predicted: &C, corrected: &C, t: f32) -> C {
        let kind = ComponentKind::of::<C>();
        let erased_fns = self
            .fns_map
            .get(&kind)
            .context("the component is not part of the protocol")
            .unwrap();
        let fns = unsafe { erased_fns.typed::<C>() };
        let correction_fn = fns.correction.unwrap();
        correction_fn(predicted, corrected, t)
    }

    pub(crate) fn interpolate<C: Component>(&self, start: &C, end: &C, t: f32) -> C {
        let kind = ComponentKind::of::<C>();
        let erased_fns = self
            .fns_map
            .get(&kind)
            .context("the component is not part of the protocol")
            .unwrap();
        let fns = unsafe { erased_fns.typed::<C>() };
        let interpolation_fn = fns.interpolation.unwrap();
        interpolation_fn(start, end, t)
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
        (erased_fns.write)(self, reader, net_id, entity_world_mut, entity_map, events)
    }

    pub(crate) fn write<C: Component>(
        &self,
        reader: &mut ReadWordBuffer,
        net_id: ComponentNetId,
        entity_world_mut: &mut EntityWorldMut,
        entity_map: &mut EntityMap,
        events: &mut ConnectionEvents,
    ) -> anyhow::Result<()> {
        let component = self.raw_deserialize::<C>(reader, net_id, entity_map)?;
        let entity = entity_world_mut.id();
        // TODO: do we need the tick information in the event?
        let tick = Tick(0);
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

fn register_component_send<C: Component>(app: &mut App, direction: ChannelDirection) {
    let is_client = app.world.get_resource::<ClientConfig>().is_some();
    let is_server = app.world.get_resource::<ServerConfig>().is_some();
    match direction {
        ChannelDirection::ClientToServer => {
            if is_client {
                register_replicate_component_send::<C, client::ConnectionManager>(app);
            }
            if is_server {
                crate::server::events::emit_replication_events::<C>(app);
            }
        }
        ChannelDirection::ServerToClient => {
            if is_server {
                register_replicate_component_send::<C, server::ConnectionManager>(app);
            }
            if is_client {
                crate::client::events::emit_replication_events::<C>(app);
            }
        }
        ChannelDirection::Bidirectional => {
            register_component_send::<C>(app, ChannelDirection::ServerToClient);
            register_component_send::<C>(app, ChannelDirection::ClientToServer);
        }
    }
}

fn register_resource_send<R: Resource + Message>(app: &mut App, direction: ChannelDirection) {
    let is_client = app.world.get_resource::<ClientConfig>().is_some();
    let is_server = app.world.get_resource::<ServerConfig>().is_some();
    match direction {
        ChannelDirection::ClientToServer => {
            if is_client {
                crate::shared::replication::resources::send::add_resource_send_systems::<
                    client::ConnectionManager,
                    R,
                >(app);
            }
            if is_server {
                crate::shared::replication::resources::receive::add_resource_receive_systems::<
                    server::ConnectionManager,
                    R,
                >(app);
            }
        }
        ChannelDirection::ServerToClient => {
            if is_server {
                crate::shared::replication::resources::send::add_resource_send_systems::<
                    server::ConnectionManager,
                    R,
                >(app);
            }
            if is_client {
                crate::shared::replication::resources::receive::add_resource_receive_systems::<
                    client::ConnectionManager,
                    R,
                >(app);
            }
        }
        ChannelDirection::Bidirectional => {
            register_resource_send::<R>(app, ChannelDirection::ClientToServer);
            register_resource_send::<R>(app, ChannelDirection::ServerToClient);
        }
    }
}

/// Add a component to the list of components that can be sent
pub trait AppComponentExt {
    fn register_component<C: Component + Message>(&mut self, direction: ChannelDirection);

    fn register_resource<R: Resource + Message>(&mut self, direction: ChannelDirection);

    fn add_component_map_entities<C: MapEntities + 'static>(&mut self);
    fn add_prediction<C: SyncComponent>(&mut self, prediction_mode: ComponentSyncMode);
    fn add_linear_correction_fn<C: SyncComponent + Linear>(&mut self);

    fn add_correction_fn<C: SyncComponent>(&mut self, correction_fn: LerpFn<C>);
    fn add_interpolation<C: SyncComponent>(&mut self, interpolation_mode: ComponentSyncMode);
    fn add_linear_interpolation_fn<C: SyncComponent + Linear>(&mut self);

    fn add_interpolation_fn<C: SyncComponent>(&mut self, interpolation_fn: LerpFn<C>);
}

impl AppComponentExt for App {
    fn register_component<C: Component + Message>(&mut self, direction: ChannelDirection) {
        let mut registry = self.world.resource_mut::<ComponentRegistry>();
        registry.register_component::<C>();
        register_component_send::<C>(self, direction);
    }

    fn register_resource<R: Resource + Message>(&mut self, direction: ChannelDirection) {
        self.register_component::<ReplicateResource<R>>(direction);
        register_resource_send::<R>(self, direction)
    }

    fn add_component_map_entities<C: MapEntities + 'static>(&mut self) {
        let mut registry = self.world.resource_mut::<ComponentRegistry>();

        registry.add_map_entities::<C>();
    }

    fn add_prediction<C: SyncComponent>(&mut self, prediction_mode: ComponentSyncMode) {
        let mut registry = self.world.resource_mut::<ComponentRegistry>();
        registry.set_prediction_mode::<C>(prediction_mode);

        // TODO: make prediction/interpolation possible on server?
        let is_client = self.world.get_resource::<ClientConfig>().is_some();
        if is_client {
            add_prediction_systems::<C>(self, prediction_mode);
        }
    }

    fn add_linear_correction_fn<C: SyncComponent + Linear>(&mut self) {
        let mut registry = self.world.resource_mut::<ComponentRegistry>();
        registry.set_linear_correction::<C>();
        // TODO: register correction systems only if correction is enabled?
    }

    fn add_correction_fn<C: SyncComponent>(&mut self, correction_fn: LerpFn<C>) {
        let mut registry = self.world.resource_mut::<ComponentRegistry>();
        registry.set_correction::<C>(correction_fn);
    }

    fn add_interpolation<C: SyncComponent>(&mut self, interpolation_mode: ComponentSyncMode) {
        let mut registry = self.world.resource_mut::<ComponentRegistry>();
        registry.set_interpolation_mode::<C>(interpolation_mode);
        // TODO: make prediction/interpolation possible on server?
        let is_client = self.world.get_resource::<ClientConfig>().is_some();
        if is_client {
            add_prepare_interpolation_systems::<C>(self, interpolation_mode);
            if interpolation_mode == ComponentSyncMode::Full {
                // TODO: handle custom interpolation
                add_interpolation_systems::<C>(self);
            }
        }
    }

    fn add_linear_interpolation_fn<C: SyncComponent + Linear>(&mut self) {
        let mut registry = self.world.resource_mut::<ComponentRegistry>();
        registry.set_linear_interpolation::<C>();
    }

    fn add_interpolation_fn<C: SyncComponent>(&mut self, interpolation_fn: LerpFn<C>) {
        let mut registry = self.world.resource_mut::<ComponentRegistry>();
        registry.set_interpolation::<C>(interpolation_fn);
    }
}

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
    pub fn of<C: 'static>() -> Self {
        Self(TypeId::of::<C>())
    }
}

impl TypeKind for ComponentKind {}

impl From<TypeId> for ComponentKind {
    fn from(type_id: TypeId) -> Self {
        Self(type_id)
    }
}
