use anyhow::Context;
use bevy::app::PreUpdate;
use bevy::ecs::entity::MapEntities;
use std::any::TypeId;
use std::fmt::{Debug, Display};
use std::hash::Hash;
use std::ops::{Add, Mul};

use bevy::prelude::{
    App, Component, DetectChangesMut, Entity, EntityMapper, EntityWorldMut, IntoSystemConfigs,
    Resource, TypePath, World,
};
use bevy::reflect::{FromReflect, GetTypeRegistration};
use bevy::utils::HashMap;
use cfg_if::cfg_if;

use bitcode::encoding::Fixed;
use bitcode::Encode;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use tracing::{debug, error, trace, warn};

use crate::client::components::{ComponentSyncMode, SyncMetadata};
use crate::client::config::ClientConfig;
use crate::client::interpolation::{add_interpolation_systems, add_prepare_interpolation_systems};
use crate::client::prediction::plugin::add_prediction_systems;
use crate::prelude::client::SyncComponent;
use crate::prelude::server::{ServerConfig, ServerPlugins};
use crate::prelude::{
    client, server, AppMessageExt, ChannelDirection, Message, MessageRegistry,
    PreSpawnedPlayerObject, RemoteEntityMap, ReplicateResourceMetadata, Tick,
};
use crate::protocol::message::{MessageKind, MessageRegistration, MessageType};
use crate::protocol::registry::{NetId, TypeKind, TypeMapper};
use crate::protocol::serialize::{ErasedSerializeFns, MapEntitiesFn, SerializeFns};
use crate::protocol::{BitSerializable, EventContext};
use crate::serialize::bitcode::reader::BitcodeReader;
use crate::serialize::bitcode::writer::BitcodeWriter;
use crate::serialize::reader::ReadBuffer;
use crate::serialize::writer::WriteBuffer;
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

/// A [`Resource`] that will keep track of all the [`Components`](Component) that can be replicated.
///
///
/// ### Adding Components
///
/// You register components by calling the [`register_component`](AppComponentExt::register_component) method directly on the App.
/// You can provide a [`ChannelDirection`] to specify if the component should be sent from the client to the server, from the server to the client, or both.
///
/// ```rust
/// use bevy::prelude::*;
/// use serde::{Deserialize, Serialize};
/// use lightyear::prelude::*;
///
/// #[derive(Component, Serialize, Deserialize)]
/// struct MyComponent;
///
/// fn add_components(app: &mut App) {
///   app.register_component::<MyComponent>(ChannelDirection::Bidirectional);
/// }
/// ```
///
/// ### Customizing Component behaviour
///
/// There are some cases where you might want to define additional behaviour for a component.
///
/// #### Entity Mapping
/// If the component contains [`Entities`](Entity), you need to specify how those entities
/// will be mapped from the remote world to the local world.
///
/// Provided that your type implements [`MapEntities`], you can extend the protocol to support this behaviour, by
/// calling the [`add_map_entities`](ComponentRegistration::add_map_entities) method.
///
/// #### Prediction
/// When client-prediction is enabled, we create two distinct entities on the client when the server replicates an entity: a Confirmed entity and a Predicted entity.
/// The Confirmed entity will just get updated when the client receives the server updates, while the Predicted entity will be updated by the client's prediction system.
///
/// Components are not synced from the Confirmed entity to the Predicted entity by default, you have to enable this behaviour.
/// You can do this by calling the [`add_prediction`](ComponentRegistration::add_prediction) method.
/// You will have to provide a [`ComponentSyncMode`] that defines the behaviour of the prediction system.
///
/// #### Correction
/// When client-prediction is enabled, there might be cases where there is a mismatch between the state of the Predicted entity
/// and the state of the Confirmed entity. In this case, we rollback by snapping the Predicted entity to the Confirmed entity and replaying the last few frames.
///
/// However, rollbacks that do an instant update can be visually jarring, so we provide the option to smooth the rollback process over a few frames.
/// You can do this by calling the [`add_correction_fn`](ComponentRegistration::add_correction_fn) method.
///
/// If your component implements the [`Linear`] trait, you can use the [`add_linear_correction_fn`](ComponentRegistration::add_linear_correction_fn) method,
/// which provides linear interpolation.
///
/// #### Interpolation
/// Similarly to client-prediction, we create two distinct entities on the client when the server replicates an entity: a Confirmed entity and an Interpolated entity.
/// The Confirmed entity will just get updated when the client receives the server updates, while the Interpolated entity will be updated by the client's interpolation system,
/// which will interpolate between two Confirmed states.
///
/// Components are not synced from the Confirmed entity to the Interpolated entity by default, you have to enable this behaviour.
/// You can do this by calling the [`add_interpolation`](ComponentRegistration::add_interpolation) method.
/// You will have to provide a [`ComponentSyncMode`] that defines the behaviour of the interpolation system.
///
/// You will also need to provide an interpolation function that will be used to interpolate between two states.
/// If your component implements the [`Linear`] trait, you can use the [`add_linear_interpolation_fn`](ComponentRegistration::add_linear_interpolation_fn) method,
/// which means that we will interpolate using linear interpolation.
///
/// You can also use your own interpolation function by using the [`add_interpolation_fn`](ComponentRegistration::add_interpolation_fn) method.
///
/// ```rust
/// use bevy::prelude::*;
/// use lightyear::prelude::*;
/// use lightyear::prelude::client::*;
///
/// #[derive(Component, Clone, PartialEq, Serialize, Deserialize)]
/// struct MyComponent(f32);
///
/// fn my_lerp_fn(start: &MyComponent, other: &MyComponent, t: f32) -> MyComponent {
///    MyComponent(start.0 * (1.0 - t) + other.0 * t)
/// }
///
///
/// fn add_messages(app: &mut App) {
///   app.register_component::<MyComponent>(ChannelDirection::ServerToClient)
///       .add_prediction(ComponentSyncMode::Full)
///       .add_interpolation(ComponentSyncMode::Full)
///       .add_interpolation_fn(my_lerp_fn);
/// }
/// ```
#[derive(Debug, Default, Clone, Resource, PartialEq, TypePath)]
pub struct ComponentRegistry {
    replication_map: HashMap<ComponentKind, ReplicationMetadata>,
    interpolation_map: HashMap<ComponentKind, InterpolationMetadata>,
    prediction_map: HashMap<ComponentKind, PredictionMetadata>,
    serialize_fns_map: HashMap<ComponentKind, ErasedSerializeFns>,
    pub(crate) kind_map: TypeMapper<ComponentKind>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ReplicationMetadata {
    pub write: RawWriteFn,
    pub remove: RawRemoveFn,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PredictionMetadata {
    pub prediction_mode: ComponentSyncMode,
    pub correction: Option<unsafe fn()>,
    /// Function used to compare the confirmed component with the predicted component's history
    /// to determine if a rollback is needed.
    /// Will default to a PartialEq implementation, but can be overriden.
    pub rollback_check: unsafe fn(),
}

impl PredictionMetadata {
    fn default_from<C: PartialEq>(mode: ComponentSyncMode) -> Self {
        let equality_check: RollbackCheckFn<C> = <C as PartialEq>::eq;
        Self {
            prediction_mode: mode,
            correction: None,
            rollback_check: unsafe { std::mem::transmute(equality_check) },
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct InterpolationMetadata {
    pub interpolation_mode: ComponentSyncMode,
    pub interpolation: Option<unsafe fn()>,
}

type RawRemoveFn = fn(&ComponentRegistry, &mut EntityWorldMut);
type RawWriteFn = fn(
    &ComponentRegistry,
    &mut BitcodeReader,
    ComponentNetId,
    &mut EntityWorldMut,
    &mut EntityMap,
    &mut ConnectionEvents,
) -> anyhow::Result<()>;

/// Function used to interpolate from one component state (`start`) to another (`other`)
/// t goes from 0.0 (`start`) to 1.0 (`other`)
pub type LerpFn<C> = fn(start: &C, other: &C, t: f32) -> C;

/// Function used to check if a rollback is needed, by comparing the server's value with the client's predicted value.
/// Defaults to PartialEq::eq
type RollbackCheckFn<C> = fn(this: &C, that: &C) -> bool;

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

impl ComponentRegistry {
    pub fn net_id<C: Component>(&self) -> ComponentNetId {
        self.kind_map
            .net_id(&ComponentKind::of::<C>())
            .copied()
            .unwrap_or_else(|| panic!("Component {} is not registered", std::any::type_name::<C>()))
    }
    pub fn get_net_id<C: Component>(&self) -> Option<ComponentNetId> {
        self.kind_map.net_id(&ComponentKind::of::<C>()).copied()
    }

    pub fn is_registered<C: Component>(&self) -> bool {
        self.kind_map.net_id(&ComponentKind::of::<C>()).is_some()
    }

    /// Check that the protocol is correct:
    /// - emits warnings for every component that has prediction/interpolation metadata but wasn't registered
    pub fn check(&self) {
        let mut errors = false;
        for component_kind in self.prediction_map.keys() {
            if !self.serialize_fns_map.contains_key(component_kind) {
                errors = true;
                error!("A component has prediction metadata but wasn't registered");
            }
        }
        for component_kind in self.interpolation_map.keys() {
            if !self.serialize_fns_map.contains_key(component_kind) {
                errors = true;
                error!("A component has interpolation metadata but wasn't registered");
            }
        }
        if errors {
            panic!("Detected some errors in the ComponentRegistry");
        }
    }

    pub(crate) fn register_component<C: Component + Message>(&mut self) {
        let component_kind = self.kind_map.add::<C>();
        self.serialize_fns_map
            .insert(component_kind, ErasedSerializeFns::new::<C>());
        let write: RawWriteFn = Self::write::<C>;
        let remove: RawRemoveFn = Self::remove::<C>;
        self.replication_map
            .insert(component_kind, ReplicationMetadata { write, remove });
    }

    pub(crate) fn try_add_map_entities<C: MapEntities + 'static>(&mut self) {
        let kind = ComponentKind::of::<C>();
        if let Some(erased_fns) = self.serialize_fns_map.get_mut(&kind) {
            erased_fns.add_map_entities::<C>();
        }
    }

    pub(crate) fn add_map_entities<C: MapEntities + 'static>(&mut self) {
        let kind = ComponentKind::of::<C>();
        let erased_fns = self.serialize_fns_map.get_mut(&kind).unwrap_or_else(|| {
            panic!(
                "Component {} is not part of the protocol",
                std::any::type_name::<C>()
            )
        });
        erased_fns.add_map_entities::<C>();
    }

    pub(crate) fn set_prediction_mode<C: SyncComponent>(&mut self, mode: ComponentSyncMode) {
        let kind = ComponentKind::of::<C>();
        let default_equality_fn = <C as PartialEq>::eq;
        self.prediction_map
            .entry(kind)
            .or_insert_with(|| PredictionMetadata::default_from::<C>(mode));
    }

    pub(crate) fn set_rollback_check<C: Component + PartialEq>(
        &mut self,
        rollback_check: RollbackCheckFn<C>,
    ) {
        let kind = ComponentKind::of::<C>();
        self.prediction_map
            .entry(kind)
            .or_insert_with(|| PredictionMetadata::default_from::<C>(ComponentSyncMode::Full))
            .rollback_check = unsafe { std::mem::transmute(rollback_check) };
    }

    pub(crate) fn set_linear_correction<C: Component + Linear + PartialEq>(&mut self) {
        self.set_correction(<C as Linear>::lerp);
    }

    pub(crate) fn set_correction<C: Component + PartialEq>(&mut self, correction_fn: LerpFn<C>) {
        let kind = ComponentKind::of::<C>();
        self.prediction_map
            .entry(kind)
            .or_insert_with(|| PredictionMetadata::default_from::<C>(ComponentSyncMode::Full))
            .correction = Some(unsafe { std::mem::transmute(correction_fn) });
    }

    pub(crate) fn set_interpolation_mode<C: Component>(&mut self, mode: ComponentSyncMode) {
        let kind = ComponentKind::of::<C>();
        self.interpolation_map
            .entry(kind)
            .or_insert_with(|| InterpolationMetadata {
                interpolation_mode: mode,
                interpolation: None,
            });
    }

    pub(crate) fn set_linear_interpolation<C: Component + Linear>(&mut self) {
        self.set_interpolation(<C as Linear>::lerp);
    }

    pub(crate) fn set_interpolation<C: Component>(&mut self, interpolation_fn: LerpFn<C>) {
        let kind = ComponentKind::of::<C>();
        self.interpolation_map
            .entry(kind)
            .or_insert_with(|| InterpolationMetadata {
                interpolation_mode: ComponentSyncMode::Full,
                interpolation: None,
            })
            .interpolation = Some(unsafe { std::mem::transmute(interpolation_fn) });
    }

    pub(crate) fn serialize<C: Component>(
        &self,
        component: &C,
        writer: &mut BitcodeWriter,
    ) -> anyhow::Result<RawData> {
        let kind = ComponentKind::of::<C>();
        let erased_fns = self
            .serialize_fns_map
            .get(&kind)
            .context("the component is not part of the protocol")?;
        let net_id = self.kind_map.net_id(&kind).unwrap();
        writer.start_write();
        writer.encode(net_id, Fixed)?;
        erased_fns.serialize(component, writer)?;
        Ok(writer.finish_write().to_vec())
    }

    /// Deserialize only the component value (the ComponentNetId has already been read)
    fn raw_deserialize<C: Component>(
        &self,
        reader: &mut BitcodeReader,
        net_id: ComponentNetId,
        entity_map: &mut EntityMap,
    ) -> anyhow::Result<C> {
        let kind = self
            .kind_map
            .kind(net_id)
            .context("unknown component kind")?;
        let erased_fns = self
            .serialize_fns_map
            .get(kind)
            .context("the component is not part of the protocol")?;
        erased_fns.deserialize(reader, entity_map)
    }

    pub(crate) fn deserialize<C: Component>(
        &self,
        reader: &mut BitcodeReader,
        entity_map: &mut EntityMap,
    ) -> anyhow::Result<C> {
        let net_id = reader.decode::<ComponentNetId>(Fixed)?;
        self.raw_deserialize(reader, net_id, entity_map)
    }

    pub(crate) fn map_entities<C: 'static>(&self, component: &mut C, entity_map: &mut EntityMap) {
        let kind = ComponentKind::of::<C>();
        let erased_fns = self
            .serialize_fns_map
            .get(&kind)
            .context("the component is not part of the protocol")
            .unwrap();
        erased_fns.map_entities(component, entity_map)
    }

    pub(crate) fn prediction_mode<C: Component>(&self) -> ComponentSyncMode {
        let kind = ComponentKind::of::<C>();
        self.prediction_map
            .get(&kind)
            .context("the component is not part of the protocol")
            .map_or(ComponentSyncMode::None, |metadata| metadata.prediction_mode)
    }

    pub(crate) fn interpolation_mode<C: Component>(&self) -> ComponentSyncMode {
        let kind = ComponentKind::of::<C>();
        self.interpolation_map
            .get(&kind)
            .context("the component is not part of the protocol")
            .map_or(ComponentSyncMode::None, |metadata| {
                metadata.interpolation_mode
            })
    }

    pub(crate) fn has_correction<C: Component>(&self) -> bool {
        let kind = ComponentKind::of::<C>();
        self.prediction_map
            .get(&kind)
            .context("the component is not part of the protocol")
            .map_or(false, |metadata| metadata.correction.is_some())
    }
    pub(crate) fn correct<C: Component>(&self, predicted: &C, corrected: &C, t: f32) -> C {
        let kind = ComponentKind::of::<C>();
        let prediction_metadata = self
            .prediction_map
            .get(&kind)
            .context("the component is not part of the protocol")
            .unwrap();
        let correction_fn: LerpFn<C> =
            unsafe { std::mem::transmute(prediction_metadata.correction.unwrap()) };
        correction_fn(predicted, corrected, t)
    }

    pub(crate) fn interpolate<C: Component>(&self, start: &C, end: &C, t: f32) -> C {
        let kind = ComponentKind::of::<C>();
        let interpolation_metadata = self
            .interpolation_map
            .get(&kind)
            .context("the component is not part of the protocol")
            .unwrap();
        let interpolation_fn: LerpFn<C> =
            unsafe { std::mem::transmute(interpolation_metadata.interpolation.unwrap()) };
        interpolation_fn(start, end, t)
    }

    /// SAFETY: the ReadWordBuffer must contain bytes corresponding to the correct component type
    pub(crate) fn raw_write(
        &self,
        reader: &mut BitcodeReader,
        entity_world_mut: &mut EntityWorldMut,
        entity_map: &mut EntityMap,
        events: &mut ConnectionEvents,
    ) -> anyhow::Result<()> {
        let net_id = reader.decode::<ComponentNetId>(Fixed)?;
        let kind = self
            .kind_map
            .kind(net_id)
            .context("unknown component kind")?;
        let replication_metadata = self
            .replication_map
            .get(kind)
            .context("the component is not part of the protocol")?;
        (replication_metadata.write)(self, reader, net_id, entity_world_mut, entity_map, events)
    }

    pub(crate) fn write<C: Component>(
        &self,
        reader: &mut BitcodeReader,
        net_id: ComponentNetId,
        entity_world_mut: &mut EntityWorldMut,
        entity_map: &mut EntityMap,
        events: &mut ConnectionEvents,
    ) -> anyhow::Result<()> {
        trace!("Writing component {} to entity", std::any::type_name::<C>());
        let component = self.raw_deserialize::<C>(reader, net_id, entity_map)?;
        let entity = entity_world_mut.id();
        // TODO: do we need the tick information in the event?
        let tick = Tick(0);
        // TODO: should we send the event based on on the message type (Insert/Update) or based on whether the component was actually inserted?
        if let Some(mut c) = entity_world_mut.get_mut::<C>() {
            events.push_update_component(entity, net_id, tick);
            // TODO: use set_if_neq for PartialEq
            *c = component;
        } else {
            events.push_insert_component(entity, net_id, tick);
            entity_world_mut.insert(component);
        }
        Ok(())
    }

    pub(crate) fn raw_remove(&self, net_id: ComponentNetId, entity_world_mut: &mut EntityWorldMut) {
        let kind = self.kind_map.kind(net_id).expect("unknown component kind");
        let replication_metadata = self
            .replication_map
            .get(kind)
            .expect("the component is not part of the protocol");
        (replication_metadata.remove)(self, entity_world_mut);
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
                debug!(
                    "register send events on server for {}",
                    std::any::type_name::<C>()
                );
                crate::server::events::emit_replication_events::<C>(app);
            }
        }
        ChannelDirection::ServerToClient => {
            if is_server {
                register_replicate_component_send::<C, server::ConnectionManager>(app);
            }
            if is_client {
                debug!(
                    "register send events on client for {}",
                    std::any::type_name::<C>()
                );
                crate::client::events::emit_replication_events::<C>(app);
            }
        }
        ChannelDirection::Bidirectional => {
            register_component_send::<C>(app, ChannelDirection::ServerToClient);
            register_component_send::<C>(app, ChannelDirection::ClientToServer);
        }
    }
}

/// Add a component to the list of components that can be sent
pub trait AppComponentExt {
    /// Registers the component in the Registry
    /// This component can now be sent over the network.
    fn register_component<C: Component + Message>(
        &mut self,
        direction: ChannelDirection,
    ) -> ComponentRegistration<'_, C>;

    /// Enable prediction systems for this component.
    /// You can specify the prediction [`ComponentSyncMode`]
    fn add_prediction<C: SyncComponent>(&mut self, prediction_mode: ComponentSyncMode);

    /// Add a `Correction` behaviour to this component by using a linear interpolation function.
    fn add_linear_correction_fn<C: SyncComponent + Linear>(&mut self);

    /// Add a `Correction` behaviour to this component.
    fn add_correction_fn<C: SyncComponent>(&mut self, correction_fn: LerpFn<C>);

    /// Add a custom function to use for checking if a rollback is needed.
    /// (By default we use the PartialEq::eq function, but you can use this to override the
    ///  equality check. For example, you might want to add a threshold for floating point numbers)
    fn add_rollback_check<C: SyncComponent>(&mut self, rollback_check: RollbackCheckFn<C>);

    /// Register helper systems to perform interpolation for the component; but the user has to define the interpolation logic
    /// themselves (the interpolation_fn will not be used)
    fn add_custom_interpolation<C: SyncComponent>(&mut self, interpolation_mode: ComponentSyncMode);

    /// Enable interpolation systems for this component.
    /// You can specify the interpolation [`ComponentSyncMode`]
    fn add_interpolation<C: SyncComponent>(&mut self, interpolation_mode: ComponentSyncMode);

    /// Add a `Interpolation` behaviour to this component by using a linear interpolation function.
    fn add_linear_interpolation_fn<C: SyncComponent + Linear>(&mut self);

    /// Add a `Interpolation` behaviour to this component.
    fn add_interpolation_fn<C: SyncComponent>(&mut self, interpolation_fn: LerpFn<C>);
}

pub struct ComponentRegistration<'a, C> {
    app: &'a mut App,
    _phantom: std::marker::PhantomData<C>,
}

impl<C> ComponentRegistration<'_, C> {
    /// Specify that the component contains entities which should be mapped from the remote world to the local world
    /// upon deserialization
    pub fn add_map_entities(self) -> Self
    where
        C: MapEntities + 'static,
    {
        let mut registry = self.app.world.resource_mut::<ComponentRegistry>();
        registry.add_map_entities::<C>();
        self
    }

    /// Enable prediction systems for this component.
    /// You can specify the prediction [`ComponentSyncMode`]
    pub fn add_prediction(self, prediction_mode: ComponentSyncMode) -> Self
    where
        C: SyncComponent,
    {
        self.app.add_prediction::<C>(prediction_mode);
        self
    }

    /// Add a `Correction` behaviour to this component by using a linear interpolation function.
    pub fn add_linear_correction_fn(self) -> Self
    where
        C: SyncComponent + Linear,
    {
        self.app.add_linear_correction_fn::<C>();
        self
    }

    /// Add a `Correction` behaviour to this component.
    pub fn add_correction_fn(self, correction_fn: LerpFn<C>) -> Self
    where
        C: SyncComponent,
    {
        self.app.add_correction_fn::<C>(correction_fn);
        self
    }

    /// Add a custom function to use for checking if a rollback is needed.
    /// (By default we use the PartialEq::eq function, but you can use this to override the
    ///  equality check. For example, you might want to add a threshold for floating point numbers)
    pub fn add_rollback_check(self, rollback_check: RollbackCheckFn<C>) -> Self
    where
        C: SyncComponent,
    {
        self.app.add_rollback_check::<C>(rollback_check);
        self
    }

    /// Enable interpolation systems for this component.
    /// You can specify the interpolation [`ComponentSyncMode`]
    pub fn add_interpolation(self, interpolation_mode: ComponentSyncMode) -> Self
    where
        C: SyncComponent,
    {
        self.app.add_interpolation::<C>(interpolation_mode);
        self
    }

    /// Register helper systems to perform interpolation for the component; but the user has to define the interpolation logic
    /// themselves (the interpolation_fn will not be used)
    pub fn add_custom_interpolation(self, interpolation_mode: ComponentSyncMode) -> Self
    where
        C: SyncComponent,
    {
        self.app.add_custom_interpolation::<C>(interpolation_mode);
        self
    }

    /// Add a `Interpolation` behaviour to this component by using a linear interpolation function.
    pub fn add_linear_interpolation_fn(self) -> Self
    where
        C: SyncComponent + Linear,
    {
        self.app.add_linear_interpolation_fn::<C>();
        self
    }

    /// Add a `Interpolation` behaviour to this component.
    pub fn add_interpolation_fn(self, interpolation_fn: LerpFn<C>) -> Self
    where
        C: SyncComponent,
    {
        self.app.add_interpolation_fn::<C>(interpolation_fn);
        self
    }
}

impl AppComponentExt for App {
    fn register_component<C: Component + Message>(
        &mut self,
        direction: ChannelDirection,
    ) -> ComponentRegistration<'_, C> {
        let mut registry = self.world.resource_mut::<ComponentRegistry>();
        if !registry.is_registered::<C>() {
            registry.register_component::<C>();
        }
        debug!("register component {}", std::any::type_name::<C>());
        register_component_send::<C>(self, direction);
        ComponentRegistration {
            app: self,
            _phantom: std::marker::PhantomData,
        }
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

    fn add_rollback_check<C: SyncComponent>(&mut self, rollback_check: RollbackCheckFn<C>) {
        let mut registry = self.world.resource_mut::<ComponentRegistry>();
        registry.set_rollback_check::<C>(rollback_check);
    }

    fn add_custom_interpolation<C: SyncComponent>(
        &mut self,
        interpolation_mode: ComponentSyncMode,
    ) {
        let mut registry = self.world.resource_mut::<ComponentRegistry>();
        registry.set_interpolation_mode::<C>(interpolation_mode);
        // TODO: make prediction/interpolation possible on server?
        let is_client = self.world.get_resource::<ClientConfig>().is_some();
        if is_client {
            add_prepare_interpolation_systems::<C>(self, interpolation_mode);
        }
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
