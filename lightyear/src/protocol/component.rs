use bevy::ecs::component::ComponentId;
use bevy::ecs::entity::MapEntities;
use std::any::TypeId;
use std::fmt::Debug;
use std::hash::Hash;
use std::ops::{Add, Mul};

use bevy::prelude::{App, Component, EntityWorldMut, Mut, Resource, TypePath, World};
use bevy::ptr::Ptr;
use bevy::utils::HashMap;
use serde::de::DeserializeOwned;
use serde::Serialize;

use tracing::{debug, error, trace};

use crate::client::components::ComponentSyncMode;
use crate::client::config::ClientConfig;
use crate::client::interpolation::{add_interpolation_systems, add_prepare_interpolation_systems};
use crate::client::prediction::plugin::{
    add_non_networked_rollback_systems, add_prediction_systems, add_resource_rollback_systems,
};
use crate::prelude::client::SyncComponent;
use crate::prelude::server::ServerConfig;
use crate::prelude::{ChannelDirection, Message, Tick};
use crate::protocol::delta::ErasedDeltaFns;
use crate::protocol::registry::{NetId, TypeKind, TypeMapper};
use crate::protocol::serialize::{ErasedSerializeFns, SerializeFns};
use crate::serialize::reader::Reader;
use crate::serialize::SerializationError;
use crate::shared::events::connection::ConnectionEvents;
use crate::shared::replication::delta::{DeltaMessage, Diffable};
use crate::shared::replication::entity_map::{EntityMap, ReceiveEntityMap};

pub type ComponentNetId = NetId;

#[derive(thiserror::Error, Debug)]
pub enum ComponentError {
    #[error("component is not registered in the protocol")]
    NotRegistered,
    #[error("missing replication functions for component")]
    MissingReplicationFns,
    #[error("missing serialization functions for component")]
    MissingSerializationFns,
    #[error("missing delta compression functions for component")]
    MissingDeltaFns,
    #[error("delta compression error: {0}")]
    DeltaCompressionError(String),
    #[error("component error: {0}")]
    SerializationError(#[from] SerializationError),
}

/// A [`Resource`] that will keep track of all the [`Components`](Component) that can be replicated.
///
///
/// ### Adding Components
///
/// You register components by calling the [`register_component`](AppComponentExt::register_component) method directly on the App.
/// You can provide a [`ChannelDirection`] to specify if the component should be sent from the client to the server, from the server to the client, or both.
///
/// A component needs to implement the `Serialize`, `Deserialize` and `PartialEq` traits.
///
/// ```rust
/// use bevy::prelude::*;
/// use serde::{Deserialize, Serialize};
/// use lightyear::prelude::*;
///
/// #[derive(Component, PartialEq, Serialize, Deserialize)]
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
/// If the component contains [`Entities`](bevy::prelude::Entity), you need to specify how those entities
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
    pub(crate) replication_map: HashMap<ComponentKind, ReplicationMetadata>,
    interpolation_map: HashMap<ComponentKind, InterpolationMetadata>,
    prediction_map: HashMap<ComponentKind, PredictionMetadata>,
    serialize_fns_map: HashMap<ComponentKind, ErasedSerializeFns>,
    delta_fns_map: HashMap<ComponentKind, ErasedDeltaFns>,
    pub(crate) kind_map: TypeMapper<ComponentKind>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ReplicationMetadata {
    pub component_id: ComponentId,
    pub delta_compression_id: ComponentId,
    pub replicate_once_id: ComponentId,
    pub override_target_id: ComponentId,
    pub disabled_id: ComponentId,
    pub write: RawWriteFn,
    pub remove: Option<RawRemoveFn>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PredictionMetadata {
    pub prediction_mode: ComponentSyncMode,
    pub correction: Option<unsafe fn()>,
    /// Function used to compare the confirmed component with the predicted component's history
    /// to determine if a rollback is needed. Returns true if we should do a rollback.
    /// Will default to a PartialEq::ne implementation, but can be overriden.
    pub should_rollback: unsafe fn(),
}

impl PredictionMetadata {
    fn default_from<C: PartialEq>(mode: ComponentSyncMode) -> Self {
        let should_rollback: ShouldRollbackFn<C> = <C as PartialEq>::ne;
        Self {
            prediction_mode: mode,
            correction: None,
            should_rollback: unsafe {
                std::mem::transmute::<for<'a, 'b> fn(&'a C, &'b C) -> bool, unsafe fn()>(
                    should_rollback,
                )
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct InterpolationMetadata {
    pub interpolation_mode: ComponentSyncMode,
    pub interpolation: Option<unsafe fn()>,
    pub custom_interpolation: bool,
}

type RawRemoveFn = fn(&ComponentRegistry, &mut EntityWorldMut);
type RawWriteFn = fn(
    &ComponentRegistry,
    &mut Reader,
    ComponentNetId,
    Tick,
    &mut EntityWorldMut,
    &mut ReceiveEntityMap,
    &mut ConnectionEvents,
) -> Result<(), ComponentError>;

/// Function used to interpolate from one component state (`start`) to another (`other`)
/// t goes from 0.0 (`start`) to 1.0 (`other`)
pub type LerpFn<C> = fn(start: &C, other: &C, t: f32) -> C;

/// Function that returns true if a rollback is needed, by comparing the server's value with the client's predicted value.
/// Defaults to PartialEq::ne
type ShouldRollbackFn<C> = fn(this: &C, that: &C) -> bool;

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
    pub fn net_id<C: 'static>(&self) -> ComponentNetId {
        self.kind_map
            .net_id(&ComponentKind::of::<C>())
            .copied()
            .unwrap_or_else(|| panic!("Component {} is not registered", std::any::type_name::<C>()))
    }
    pub fn get_net_id<C: 'static>(&self) -> Option<ComponentNetId> {
        self.kind_map.net_id(&ComponentKind::of::<C>()).copied()
    }

    /// Return the name of the component from the [`ComponentKind`]
    pub fn name(&self, kind: ComponentKind) -> &'static str {
        self.serialize_fns_map.get(&kind).unwrap().type_name
    }

    pub fn is_registered<C: 'static>(&self) -> bool {
        self.kind_map.net_id(&ComponentKind::of::<C>()).is_some()
    }

    /// Check that the protocol is correct:
    /// - emits warnings for every component that has prediction/interpolation metadata but wasn't registered
    pub fn check(&self) {
        for component_kind in self.prediction_map.keys() {
            if !self.serialize_fns_map.contains_key(component_kind) {
                panic!(
                    "A component has prediction metadata but wasn't registered for serialization"
                );
            }
        }
        for (component_kind, interpolation_data) in &self.interpolation_map {
            if !self.serialize_fns_map.contains_key(component_kind) {
                panic!("A component has interpolation metadata but wasn't registered for serialization");
            } else if interpolation_data.interpolation_mode == ComponentSyncMode::Full
                && interpolation_data.interpolation.is_none()
                && !interpolation_data.custom_interpolation
            {
                let name = self
                    .serialize_fns_map
                    .get(component_kind)
                    .unwrap()
                    .type_name;
                panic!("The Component {name:?} was registered for interpolation with ComponentSyncMode::FULL but no interpolation function was provided!");
            }
        }
    }

    pub(crate) fn register_component<C: Message + Serialize + DeserializeOwned>(&mut self) {
        let component_kind = self.kind_map.add::<C>();
        self.serialize_fns_map
            .insert(component_kind, ErasedSerializeFns::new::<C>());
    }

    pub(crate) fn register_component_custom_serde<C: Message>(
        &mut self,
        serialize_fns: SerializeFns<C>,
    ) {
        let component_kind = self.kind_map.add::<C>();
        self.serialize_fns_map.insert(
            component_kind,
            ErasedSerializeFns::new_custom_serde::<C>(serialize_fns),
        );
    }
}

mod serialize {
    use super::*;
    use crate::serialize::reader::Reader;
    use crate::serialize::writer::Writer;
    use crate::serialize::ToBytes;
    use crate::shared::replication::entity_map::SendEntityMap;

    impl ComponentRegistry {
        pub(crate) fn try_add_map_entities<C: Clone + MapEntities + 'static>(&mut self) {
            let kind = ComponentKind::of::<C>();
            if let Some(erased_fns) = self.serialize_fns_map.get_mut(&kind) {
                erased_fns.add_map_entities::<C>();
            }
        }

        pub(crate) fn add_map_entities<C: Clone + MapEntities + 'static>(&mut self) {
            let kind = ComponentKind::of::<C>();
            let erased_fns = self.serialize_fns_map.get_mut(&kind).unwrap_or_else(|| {
                panic!(
                    "Component {} is not part of the protocol",
                    std::any::type_name::<C>()
                )
            });
            erased_fns.add_map_entities::<C>();
        }

        /// Returns true if we have a registered `map_entities` function for this component type
        pub(crate) fn is_map_entities<C: 'static>(&self) -> bool {
            let kind = ComponentKind::of::<C>();
            let erased_fns = self
                .serialize_fns_map
                .get(&kind)
                .expect("the component is not part of the protocol");
            erased_fns.map_entities.is_some()
        }

        /// Returns true if we have a registered `map_entities` function for this component type
        pub(crate) fn erased_is_map_entities(&self, kind: ComponentKind) -> bool {
            let erased_fns = self
                .serialize_fns_map
                .get(&kind)
                .expect("the component is not part of the protocol");
            erased_fns.map_entities.is_some()
        }

        pub(crate) fn serialize<C: 'static>(
            &self,
            component: &mut C,
            writer: &mut Writer,
            entity_map: Option<&mut SendEntityMap>,
        ) -> Result<(), ComponentError> {
            let kind = ComponentKind::of::<C>();
            let erased_fns = self
                .serialize_fns_map
                .get(&kind)
                .ok_or(ComponentError::MissingSerializationFns)?;
            let net_id = self.kind_map.net_id(&kind).unwrap();

            net_id.to_bytes(writer)?;
            // SAFETY: the ErasedFns corresponds to type C
            unsafe {
                erased_fns.serialize(component, writer, entity_map)?;
            }
            Ok(())
        }

        /// SAFETY: the Ptr must correspond to the correct ComponentKind
        pub(crate) fn erased_serialize(
            &self,
            component: Ptr,
            writer: &mut Writer,
            kind: ComponentKind,
            entity_map: Option<&mut SendEntityMap>,
        ) -> Result<(), ComponentError> {
            let erased_fns = self
                .serialize_fns_map
                .get(&kind)
                .ok_or(ComponentError::MissingSerializationFns)?;
            let net_id = self.kind_map.net_id(&kind).unwrap();
            net_id.to_bytes(writer)?;
            // SAFETY: the ErasedSerializeFns corresponds to type C
            unsafe {
                (erased_fns.erased_serialize)(erased_fns, component, writer, entity_map)?;
            }
            Ok(())
        }

        /// Deserialize only the component value (the ComponentNetId has already been read)
        pub(crate) fn raw_deserialize<C: 'static>(
            &self,
            reader: &mut Reader,
            net_id: ComponentNetId,
            entity_map: &mut ReceiveEntityMap,
        ) -> Result<C, ComponentError> {
            let kind = self
                .kind_map
                .kind(net_id)
                .ok_or(ComponentError::NotRegistered)?;
            let erased_fns = self
                .serialize_fns_map
                .get(kind)
                .ok_or(ComponentError::MissingSerializationFns)?;
            // SAFETY: the ErasedFns corresponds to type C
            unsafe { erased_fns.deserialize(reader, entity_map) }.map_err(Into::into)
        }

        pub(crate) fn deserialize<C: Component>(
            &self,
            reader: &mut Reader,
            entity_map: &mut ReceiveEntityMap,
        ) -> Result<C, ComponentError> {
            let net_id = NetId::from_bytes(reader).map_err(SerializationError::from)?;
            self.raw_deserialize(reader, net_id, entity_map)
        }

        pub(crate) fn map_entities<C: 'static>(
            &self,
            component: &mut C,
            entity_map: &mut EntityMap,
        ) -> Result<(), ComponentError> {
            let kind = ComponentKind::of::<C>();
            let erased_fns = self
                .serialize_fns_map
                .get(&kind)
                .ok_or(ComponentError::MissingSerializationFns)?;
            erased_fns.map_entities(component, entity_map);
            Ok(())
        }
    }
}

mod prediction {
    use super::*;

    impl ComponentRegistry {
        pub(crate) fn set_prediction_mode<C: SyncComponent>(&mut self, mode: ComponentSyncMode) {
            let kind = ComponentKind::of::<C>();
            let default_equality_fn = <C as PartialEq>::eq;
            self.prediction_map
                .entry(kind)
                .or_insert_with(|| PredictionMetadata::default_from::<C>(mode));
        }

        pub(crate) fn set_should_rollback<C: Component + PartialEq>(
            &mut self,
            should_rollback: ShouldRollbackFn<C>,
        ) {
            let kind = ComponentKind::of::<C>();
            self.prediction_map
                .entry(kind)
                .or_insert_with(|| PredictionMetadata::default_from::<C>(ComponentSyncMode::Full))
                .should_rollback = unsafe {
                std::mem::transmute::<for<'a, 'b> fn(&'a C, &'b C) -> bool, unsafe fn()>(
                    should_rollback,
                )
            };
        }

        pub(crate) fn set_linear_correction<C: Component + Linear + PartialEq>(&mut self) {
            self.set_correction(<C as Linear>::lerp);
        }

        pub(crate) fn set_correction<C: Component + PartialEq>(
            &mut self,
            correction_fn: LerpFn<C>,
        ) {
            let kind = ComponentKind::of::<C>();
            self.prediction_map
                .entry(kind)
                .or_insert_with(|| PredictionMetadata::default_from::<C>(ComponentSyncMode::Full))
                .correction = Some(unsafe {
                std::mem::transmute::<for<'a, 'b> fn(&'a C, &'b C, f32) -> C, unsafe fn()>(
                    correction_fn,
                )
            });
        }
        pub(crate) fn prediction_mode<C: Component>(&self) -> ComponentSyncMode {
            let kind = ComponentKind::of::<C>();
            self.prediction_map
                .get(&kind)
                .map_or(ComponentSyncMode::None, |metadata| metadata.prediction_mode)
        }

        pub(crate) fn has_correction<C: Component>(&self) -> bool {
            let kind = ComponentKind::of::<C>();
            self.prediction_map
                .get(&kind)
                .map_or(false, |metadata| metadata.correction.is_some())
        }

        /// Returns true if we should do a rollback
        pub(crate) fn should_rollback<C: Component>(&self, this: &C, that: &C) -> bool {
            let kind = ComponentKind::of::<C>();
            let prediction_metadata = self
                .prediction_map
                .get(&kind)
                .expect("the component is not part of the protocol");
            let should_rollback_fn: ShouldRollbackFn<C> =
                unsafe { std::mem::transmute(prediction_metadata.should_rollback) };
            should_rollback_fn(this, that)
        }

        pub(crate) fn correct<C: Component>(&self, predicted: &C, corrected: &C, t: f32) -> C {
            let kind = ComponentKind::of::<C>();
            let prediction_metadata = self
                .prediction_map
                .get(&kind)
                .expect("the component is not part of the protocol");
            let correction_fn: LerpFn<C> =
                unsafe { std::mem::transmute(prediction_metadata.correction.unwrap()) };
            correction_fn(predicted, corrected, t)
        }
    }
}

mod interpolation {
    use super::*;

    impl ComponentRegistry {
        pub(crate) fn set_interpolation_mode<C: Component>(&mut self, mode: ComponentSyncMode) {
            let kind = ComponentKind::of::<C>();
            self.interpolation_map
                .entry(kind)
                .or_insert_with(|| InterpolationMetadata {
                    interpolation_mode: mode,
                    interpolation: None,
                    custom_interpolation: false,
                })
                .interpolation_mode = mode;
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
                    custom_interpolation: false,
                })
                .interpolation = Some(unsafe {
                std::mem::transmute::<for<'a, 'b> fn(&'a C, &'b C, f32) -> C, unsafe fn()>(
                    interpolation_fn,
                )
            });
        }
        pub(crate) fn interpolation_mode<C: Component>(&self) -> ComponentSyncMode {
            let kind = ComponentKind::of::<C>();
            self.interpolation_map
                .get(&kind)
                .map_or(ComponentSyncMode::None, |metadata| {
                    metadata.interpolation_mode
                })
        }
        pub(crate) fn interpolate<C: Component>(&self, start: &C, end: &C, t: f32) -> C {
            let kind = ComponentKind::of::<C>();
            let interpolation_metadata = self
                .interpolation_map
                .get(&kind)
                .expect("the component is not part of the protocol");
            let interpolation_fn: LerpFn<C> =
                unsafe { std::mem::transmute(interpolation_metadata.interpolation.unwrap()) };
            interpolation_fn(start, end, t)
        }
    }
}

mod replication {
    use super::*;
    use crate::prelude::{
        DeltaCompression, DisabledComponent, OverrideTargetComponent, ReplicateOnceComponent,
    };
    use crate::serialize::reader::Reader;
    use crate::serialize::ToBytes;
    use crate::shared::replication::entity_map::ReceiveEntityMap;

    impl ComponentRegistry {
        pub(crate) fn set_replication_fns<C: Component + PartialEq>(&mut self, world: &mut World) {
            let kind = ComponentKind::of::<C>();
            let write: RawWriteFn = Self::write::<C>;
            let remove: RawRemoveFn = Self::remove::<C>;
            self.replication_map.insert(
                kind,
                ReplicationMetadata {
                    component_id: world.register_component::<C>(),
                    delta_compression_id: world.register_component::<DeltaCompression<C>>(),
                    replicate_once_id: world.register_component::<ReplicateOnceComponent<C>>(),
                    override_target_id: world.register_component::<OverrideTargetComponent<C>>(),
                    disabled_id: world.register_component::<DisabledComponent<C>>(),
                    write,
                    remove: Some(remove),
                },
            );
        }

        /// SAFETY: the ReadWordBuffer must contain bytes corresponding to the correct component type
        pub(crate) fn raw_write(
            &self,
            reader: &mut Reader,
            entity_world_mut: &mut EntityWorldMut,
            tick: Tick,
            entity_map: &mut ReceiveEntityMap,
            events: &mut ConnectionEvents,
        ) -> Result<ComponentKind, ComponentError> {
            let net_id = ComponentNetId::from_bytes(reader).map_err(SerializationError::from)?;
            let kind = self
                .kind_map
                .kind(net_id)
                .ok_or(ComponentError::NotRegistered)?;
            let replication_metadata = self
                .replication_map
                .get(kind)
                .ok_or(ComponentError::MissingReplicationFns)?;
            (replication_metadata.write)(
                self,
                reader,
                net_id,
                tick,
                entity_world_mut,
                entity_map,
                events,
            )?;
            Ok(*kind)
        }

        pub(crate) fn write<C: Component + PartialEq>(
            &self,
            reader: &mut Reader,
            net_id: ComponentNetId,
            tick: Tick,
            entity_world_mut: &mut EntityWorldMut,
            entity_map: &mut ReceiveEntityMap,
            events: &mut ConnectionEvents,
        ) -> Result<(), ComponentError> {
            trace!("Writing component {} to entity", std::any::type_name::<C>());
            let component = self.raw_deserialize::<C>(reader, net_id, entity_map)?;
            let entity = entity_world_mut.id();
            // TODO: should we send the event based on on the message type (Insert/Update) or based on whether the component was actually inserted?
            if let Some(mut c) = entity_world_mut.get_mut::<C>() {
                // only apply the update if the component is different, to not trigger change detection
                if c.as_ref() != &component {
                    events.push_update_component(entity, net_id, tick);
                    *c = component;
                }
            } else {
                events.push_insert_component(entity, net_id, tick);
                entity_world_mut.insert(component);
            }
            Ok(())
        }

        pub(crate) fn raw_remove(
            &self,
            net_id: ComponentNetId,
            entity_world_mut: &mut EntityWorldMut,
        ) {
            let kind = self.kind_map.kind(net_id).expect("unknown component kind");
            let replication_metadata = self
                .replication_map
                .get(kind)
                .expect("the component is not part of the protocol");
            let f = replication_metadata
                .remove
                .expect("the component does not have a remove function");
            f(self, entity_world_mut);
        }

        pub(crate) fn remove<C: Component>(&self, entity_world_mut: &mut EntityWorldMut) {
            entity_world_mut.remove::<C>();
        }
    }
}

mod delta {
    use super::*;

    use crate::shared::replication::delta::{DeltaComponentHistory, DeltaType};

    use crate::serialize::writer::Writer;
    use crate::shared::replication::entity_map::SendEntityMap;
    use std::ptr::NonNull;

    impl ComponentRegistry {
        /// Register delta compression functions for a component
        pub(crate) fn set_delta_compression<C: Component + PartialEq + Diffable>(&mut self)
        where
            C::Delta: Serialize + DeserializeOwned,
        {
            let kind = ComponentKind::of::<C>();
            let delta_kind = ComponentKind::of::<DeltaMessage<C::Delta>>();
            // add the delta as a message
            self.register_component::<DeltaMessage<C::Delta>>();
            // add delta-related type-erased functions
            self.delta_fns_map.insert(kind, ErasedDeltaFns::new::<C>());
            // add write/remove functions associated with the delta component's net_id
            // (since the serialized message will contain the delta component's net_id)
            // update the write function to use the delta compression logic
            let write: RawWriteFn = Self::write_delta::<C>;
            self.replication_map.insert(
                delta_kind,
                ReplicationMetadata {
                    // NOTE: we set these to 0 because they are never used for the DeltaMessage component
                    component_id: ComponentId::new(0),
                    delta_compression_id: ComponentId::new(0),
                    replicate_once_id: ComponentId::new(0),
                    override_target_id: ComponentId::new(0),
                    disabled_id: ComponentId::new(0),
                    write,
                    remove: None,
                },
            );
        }

        /// SAFETY: the Ptr must correspond to the correct ComponentKind
        pub(crate) unsafe fn erased_clone(
            &self,
            data: Ptr,
            kind: ComponentKind,
        ) -> Result<NonNull<u8>, ComponentError> {
            let delta_fns = self
                .delta_fns_map
                .get(&kind)
                .ok_or(ComponentError::MissingDeltaFns)?;
            Ok((delta_fns.clone)(data))
        }

        /// SAFETY: the data from the Ptr must correspond to the correct ComponentKind
        pub(crate) unsafe fn erased_drop(
            &self,
            data: NonNull<u8>,
            kind: ComponentKind,
        ) -> Result<(), ComponentError> {
            let delta_fns = self
                .delta_fns_map
                .get(&kind)
                .ok_or(ComponentError::MissingDeltaFns)?;
            (delta_fns.drop)(data);
            Ok(())
        }

        /// SAFETY: The Ptrs must correspond to the correct ComponentKind
        pub(crate) unsafe fn serialize_diff(
            &self,
            start_tick: Tick,
            start: Ptr,
            new: Ptr,
            writer: &mut Writer,
            // kind for C, not for C::Delta
            kind: ComponentKind,
            entity_map: Option<&mut SendEntityMap>,
        ) -> Result<(), ComponentError> {
            let delta_fns = self
                .delta_fns_map
                .get(&kind)
                .ok_or(ComponentError::MissingDeltaFns)?;

            let delta = (delta_fns.diff)(start_tick, start, new);
            self.erased_serialize(Ptr::new(delta), writer, delta_fns.delta_kind, entity_map)?;
            // drop the delta message
            (delta_fns.drop_delta_message)(delta);
            Ok(())
        }

        /// SAFETY: The Ptrs must correspond to the correct ComponentKind
        pub(crate) unsafe fn serialize_diff_from_base_value(
            &self,
            component_data: Ptr,
            writer: &mut Writer,
            // kind for C, not for C::Delta
            kind: ComponentKind,
            entity_map: Option<&mut SendEntityMap>,
        ) -> Result<(), ComponentError> {
            let delta_fns = self
                .delta_fns_map
                .get(&kind)
                .ok_or(ComponentError::MissingDeltaFns)?;
            let delta = (delta_fns.diff_from_base)(component_data);
            self.erased_serialize(Ptr::new(delta), writer, delta_fns.delta_kind, entity_map)?;
            // drop the delta message
            (delta_fns.drop_delta_message)(delta);
            Ok(())
        }

        /// Deserialize the DeltaMessage<C::Delta> and apply it to the component
        pub(crate) fn write_delta<C: Component + PartialEq + Diffable>(
            &self,
            reader: &mut Reader,
            net_id: ComponentNetId,
            tick: Tick,
            entity_world_mut: &mut EntityWorldMut,
            entity_map: &mut ReceiveEntityMap,
            events: &mut ConnectionEvents,
        ) -> Result<(), ComponentError> {
            trace!(
                "Writing component delta {} to entity",
                std::any::type_name::<C>()
            );
            let delta_net_id = self.net_id::<DeltaMessage<C::Delta>>();
            let delta =
                self.raw_deserialize::<DeltaMessage<C::Delta>>(reader, delta_net_id, entity_map)?;
            let entity = entity_world_mut.id();
            // TODO: should we send the event based on on the message type (Insert/Update) or based on whether the component was actually inserted?
            match delta.delta_type {
                DeltaType::Normal { previous_tick } => {
                    let Some(mut history) = entity_world_mut.get_mut::<DeltaComponentHistory<C>>()
                    else {
                        return Err(ComponentError::DeltaCompressionError(
                            format!("Entity {entity:?} does not have a ConfirmedHistory<{}>, but we received a diff for delta-compression",
                                    std::any::type_name::<C>())
                        ));
                    };
                    let Some(past_value) = history.buffer.get(&previous_tick) else {
                        return Err(ComponentError::DeltaCompressionError(
                            format!("Entity {entity:?} does not have a value for tick {previous_tick:?} in the ConfirmedHistory<{}>",
                                    std::any::type_name::<C>())
                        ));
                    };
                    // TODO: is it possible to have one clone instead of 2?
                    let mut new_value = past_value.clone();
                    new_value.apply_diff(&delta.delta);
                    // we can remove all the values strictly older than previous_tick in the component history
                    // (since we now that server has receive an ack for previous_tick)
                    history.buffer = history.buffer.split_off(&previous_tick);
                    // store the new value in the history
                    history.buffer.insert(tick, new_value.clone());
                    let Some(mut c) = entity_world_mut.get_mut::<C>() else {
                        return Err(ComponentError::DeltaCompressionError(
                            format!("Entity {entity:?} does not have a {} component, but we received a diff for delta-compression",
                            std::any::type_name::<C>())
                        ));
                    };
                    *c = new_value;
                    events.push_update_component(entity, net_id, tick);
                }
                DeltaType::FromBase => {
                    let mut new_value = C::base_value();
                    new_value.apply_diff(&delta.delta);
                    let value = new_value.clone();
                    if let Some(mut c) = entity_world_mut.get_mut::<C>() {
                        // only apply the update if the component is different, to not trigger change detection
                        if c.as_ref() != &new_value {
                            *c = new_value;
                            events.push_update_component(entity, net_id, tick);
                        }
                    } else {
                        entity_world_mut.insert(new_value);
                        events.push_insert_component(entity, net_id, tick);
                    }
                    // store the component value in the delta component history, so that we can compute
                    // diffs from it
                    if let Some(mut history) =
                        entity_world_mut.get_mut::<DeltaComponentHistory<C>>()
                    {
                        history.buffer.insert(tick, value);
                    } else {
                        // create a DeltaComponentHistory and insert the value
                        let mut history = DeltaComponentHistory::default();
                        history.buffer.insert(tick, value);
                        entity_world_mut.insert(history);
                    }
                }
            }
            Ok(())
        }
    }
}

fn register_component_send<C: Component>(app: &mut App, direction: ChannelDirection) {
    let is_client = app.world().get_resource::<ClientConfig>().is_some();
    let is_server = app.world().get_resource::<ServerConfig>().is_some();
    match direction {
        ChannelDirection::ClientToServer => {
            if is_client {
                crate::client::replication::send::register_replicate_component_send::<C>(app);
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
                crate::server::replication::send::register_replicate_component_send::<C>(app);
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
    fn register_component<C: Component + Message + Serialize + DeserializeOwned + PartialEq>(
        &mut self,
        direction: ChannelDirection,
    ) -> ComponentRegistration<'_, C>;

    /// Registers the component in the Registry: this component can now be sent over the network.
    ///
    /// You need to provide your own [`SerializeFns`]
    fn register_component_custom_serde<C: Component + Message + PartialEq>(
        &mut self,
        direction: ChannelDirection,
        serialize_fns: SerializeFns<C>,
    ) -> ComponentRegistration<'_, C>;

    /// Enable rollbacks for a component even if the component is not networked
    fn add_rollback<C: Component + PartialEq + Clone>(&mut self);

    /// Enable rollbacks for a resource.
    fn add_resource_rollback<R: Resource + Clone + Debug>(&mut self);

    /// Enable prediction systems for this component.
    /// You can specify the prediction [`ComponentSyncMode`]
    fn add_prediction<C: SyncComponent>(&mut self, prediction_mode: ComponentSyncMode);

    /// Add a `Correction` behaviour to this component by using a linear interpolation function.
    fn add_linear_correction_fn<C: SyncComponent + Linear>(&mut self);

    /// Add a `Correction` behaviour to this component.
    fn add_correction_fn<C: SyncComponent>(&mut self, correction_fn: LerpFn<C>);

    /// Add a custom function to use for checking if a rollback is needed.
    ///
    /// (By default we use the PartialEq::ne function, but you can use this to override the
    ///  equality check. For example, you might want to add a threshold for floating point numbers)
    fn add_should_rollback_fn<C: SyncComponent>(&mut self, should_rollback: ShouldRollbackFn<C>);

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

    /// Enable delta compression when serializing this component
    fn add_delta_compression<C: Component + PartialEq + Diffable>(&mut self)
    where
        C::Delta: Serialize + DeserializeOwned;
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
        C: Clone + MapEntities + 'static,
    {
        let mut registry = self.app.world_mut().resource_mut::<ComponentRegistry>();
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
    ///
    /// (By default we use the PartialEq::ne function, but you can use this to override the
    ///  equality check. For example, you might want to add a threshold for floating point numbers)
    pub fn add_should_rollback(self, should_rollback: ShouldRollbackFn<C>) -> Self
    where
        C: SyncComponent,
    {
        self.app.add_should_rollback_fn::<C>(should_rollback);
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

    /// Enable delta compression when serializing this component
    pub fn add_delta_compression(self) -> Self
    where
        C: Component + PartialEq + Diffable,
        C::Delta: Serialize + DeserializeOwned,
    {
        self.app.add_delta_compression::<C>();
        self
    }
}

impl AppComponentExt for App {
    fn register_component<C: Component + Message + PartialEq + Serialize + DeserializeOwned>(
        &mut self,
        direction: ChannelDirection,
    ) -> ComponentRegistration<'_, C> {
        self.world_mut()
            .resource_scope(|world, mut registry: Mut<ComponentRegistry>| {
                if !registry.is_registered::<C>() {
                    registry.register_component::<C>();
                }
                registry.set_replication_fns::<C>(world);
                debug!("register component {}", std::any::type_name::<C>());
            });
        register_component_send::<C>(self, direction);
        ComponentRegistration {
            app: self,
            _phantom: std::marker::PhantomData,
        }
    }

    fn register_component_custom_serde<C: Component + Message + PartialEq>(
        &mut self,
        direction: ChannelDirection,
        serialize_fns: SerializeFns<C>,
    ) -> ComponentRegistration<'_, C> {
        self.world_mut()
            .resource_scope(|world, mut registry: Mut<ComponentRegistry>| {
                if !registry.is_registered::<C>() {
                    registry.register_component_custom_serde::<C>(serialize_fns);
                }
                registry.set_replication_fns::<C>(world);
                debug!("register component {}", std::any::type_name::<C>());
            });
        register_component_send::<C>(self, direction);
        ComponentRegistration {
            app: self,
            _phantom: std::marker::PhantomData,
        }
    }

    // TODO: move this away from protocol? since it doesn't even use the registry at all
    //  maybe put this in the PredictionPlugin?
    fn add_rollback<C: Component + PartialEq + Clone>(&mut self) {
        let is_client = self.world().get_resource::<ClientConfig>().is_some();
        if is_client {
            add_non_networked_rollback_systems::<C>(self);
        }
    }

    /// Do not use `Time<Fixed>` for `R`. `Time<Fixed>` is already rollbacked.
    fn add_resource_rollback<R: Resource + Clone + Debug>(&mut self) {
        let is_client = self.world().get_resource::<ClientConfig>().is_some();
        if is_client {
            add_resource_rollback_systems::<R>(self);
        }
    }

    fn add_prediction<C: SyncComponent>(&mut self, prediction_mode: ComponentSyncMode) {
        let mut registry = self.world_mut().resource_mut::<ComponentRegistry>();
        registry.set_prediction_mode::<C>(prediction_mode);

        // TODO: make prediction/interpolation possible on server?
        let is_client = self.world().get_resource::<ClientConfig>().is_some();
        if is_client {
            add_prediction_systems::<C>(self, prediction_mode);
        }
    }

    fn add_linear_correction_fn<C: SyncComponent + Linear>(&mut self) {
        let mut registry = self.world_mut().resource_mut::<ComponentRegistry>();
        registry.set_linear_correction::<C>();
        // TODO: register correction systems only if correction is enabled?
    }

    fn add_correction_fn<C: SyncComponent>(&mut self, correction_fn: LerpFn<C>) {
        let mut registry = self.world_mut().resource_mut::<ComponentRegistry>();
        registry.set_correction::<C>(correction_fn);
    }

    fn add_should_rollback_fn<C: SyncComponent>(&mut self, rollback_check: ShouldRollbackFn<C>) {
        let mut registry = self.world_mut().resource_mut::<ComponentRegistry>();
        registry.set_should_rollback::<C>(rollback_check);
    }

    fn add_custom_interpolation<C: SyncComponent>(
        &mut self,
        interpolation_mode: ComponentSyncMode,
    ) {
        let mut registry = self.world_mut().resource_mut::<ComponentRegistry>();
        registry.set_interpolation_mode::<C>(interpolation_mode);
        let kind = ComponentKind::of::<C>();
        registry
            .interpolation_map
            .get_mut(&kind)
            .expect("the component is not part of the protocol")
            .custom_interpolation = true;

        // TODO: make prediction/interpolation possible on server?
        let is_client = self.world().get_resource::<ClientConfig>().is_some();
        if is_client {
            add_prepare_interpolation_systems::<C>(self, interpolation_mode);
        }
    }

    fn add_interpolation<C: SyncComponent>(&mut self, interpolation_mode: ComponentSyncMode) {
        let mut registry = self.world_mut().resource_mut::<ComponentRegistry>();
        registry.set_interpolation_mode::<C>(interpolation_mode);
        // TODO: make prediction/interpolation possible on server?
        let is_client = self.world().get_resource::<ClientConfig>().is_some();
        if is_client {
            add_prepare_interpolation_systems::<C>(self, interpolation_mode);
            if interpolation_mode == ComponentSyncMode::Full {
                // TODO: handle custom interpolation
                add_interpolation_systems::<C>(self);
            }
        }
    }

    fn add_linear_interpolation_fn<C: SyncComponent + Linear>(&mut self) {
        let mut registry = self.world_mut().resource_mut::<ComponentRegistry>();
        registry.set_linear_interpolation::<C>();
    }

    fn add_interpolation_fn<C: SyncComponent>(&mut self, interpolation_fn: LerpFn<C>) {
        let mut registry = self.world_mut().resource_mut::<ComponentRegistry>();
        registry.set_interpolation::<C>(interpolation_fn);
    }

    fn add_delta_compression<C: Component + PartialEq + Diffable>(&mut self)
    where
        C::Delta: Serialize + DeserializeOwned,
    {
        let mut registry = self.world_mut().resource_mut::<ComponentRegistry>();
        registry.set_delta_compression::<C>();
    }
}

/// [`ComponentKind`] is an internal wrapper around the type of the component
#[derive(Debug, Eq, Hash, Copy, Clone, PartialEq)]
pub struct ComponentKind(pub(crate) TypeId);

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::serialize::writer::Writer;
    use crate::tests::protocol::*;

    #[test]
    fn test_custom_serde() {
        let mut registry = ComponentRegistry::default();
        registry.register_component_custom_serde::<ComponentSyncModeSimple>(SerializeFns {
            serialize: serialize_component2,
            deserialize: deserialize_component2,
            serialize_map_entities: None,
        });
        let mut component = ComponentSyncModeSimple(1.0);
        let mut writer = Writer::default();
        registry
            .serialize(&mut component, &mut writer, None)
            .unwrap();
        let data = writer.to_bytes();

        let mut reader = Reader::from(data);
        let read = registry
            .deserialize(&mut reader, &mut ReceiveEntityMap::default())
            .unwrap();
        assert_eq!(component, read);
    }
}
