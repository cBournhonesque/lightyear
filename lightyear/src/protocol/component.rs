use bevy::ecs::component::ComponentId;
use bevy::ecs::entity::{EntityHash, MapEntities};
use bevy::prelude::{
    App, Component, Entity, EntityWorldMut, Mut, Reflect, Resource, TypePath, World,
};
use bevy::ptr::{OwningPtr, Ptr};
use bevy::utils::{hashbrown, HashMap};
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::alloc::Layout;
use std::any::TypeId;
use std::fmt::Debug;
use std::hash::Hash;
use std::ops::{Add, Mul};
use std::ptr::NonNull;

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
    temp_write_buffer: TempWriteBuffer,
    component_id_to_kind: HashMap<ComponentId, ComponentKind>,
    kind_to_component_id: HashMap<ComponentKind, ComponentId>,
    pub(crate) replication_map: HashMap<ComponentKind, ReplicationMetadata>,
    interpolation_map: HashMap<ComponentKind, InterpolationMetadata>,
    prediction_map: HashMap<ComponentKind, PredictionMetadata>,
    serialize_fns_map: HashMap<ComponentKind, ErasedSerializeFns>,
    delta_fns_map: HashMap<ComponentKind, ErasedDeltaFns>,
    pub(crate) kind_map: TypeMapper<ComponentKind>,
}

/// Temporary buffer to store component data that we want to insert
/// using `entity_world_mut.insert_by_ids`
#[derive(Debug, Default, Clone, PartialEq, TypePath)]
struct TempWriteBuffer {
    // temporary buffers to store the deserialized data to batch write
    // Raw storage where we can store the deserialized data bytes
    raw_bytes: Vec<u8>,
    // Positions of each component in the `raw_bytes` bufferk
    component_ptrs_indices: Vec<usize>,
    // List of component ids
    component_ids: Vec<ComponentId>,
}

impl TempWriteBuffer {
    fn batch_insert(&mut self, entity_world_mut: &mut EntityWorldMut) {
        unsafe {
            entity_world_mut.insert_by_ids(
                &self.component_ids,
                self.component_ptrs_indices.drain(..).map(|index| {
                    let ptr = NonNull::new_unchecked(self.raw_bytes.as_mut_ptr().add(index));
                    OwningPtr::new(ptr)
                }),
            )
        };
        self.raw_bytes.clear();
        self.component_ids.clear();
    }

    /// Store the component's raw bytes into a temporary buffer so that we can get an OwningPtr to it
    /// This function is called for all components that will be added to an entity, so that we can
    /// insert them all at once using `entity_world_mut.insert_by_ids`
    ///
    /// SAFETY:
    /// - the component C must match the `component_id `
    pub(crate) unsafe fn buffer_insert_raw_ptrs<C: Component>(
        &mut self,
        mut component: C,
        component_id: ComponentId,
    ) {
        let layout = Layout::new::<C>();
        let ptr = NonNull::new_unchecked(&mut component).cast::<u8>();
        // make sure the Drop trait is not called when the `component` variable goes out of scope
        std::mem::forget(component);
        let count = layout.size();
        self.raw_bytes.reserve(count);
        let space = NonNull::new_unchecked(self.raw_bytes.spare_capacity_mut()).cast::<u8>();
        space.copy_from_nonoverlapping(ptr, count);
        let length = self.raw_bytes.len();
        self.raw_bytes.set_len(length + count);
        self.component_ptrs_indices.push(length);
        self.component_ids.push(component_id);
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ReplicationMetadata {
    pub direction: ChannelDirection,
    pub delta_compression_id: ComponentId,
    pub replicate_once_id: ComponentId,
    pub override_target_id: ComponentId,
    pub write: RawWriteFn,
    pub buffer_insert_fn: RawBufferInsertFn,
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
    pub buffer_sync: SyncFn,
}

impl PredictionMetadata {
    fn default_from<C: SyncComponent>(mode: ComponentSyncMode) -> Self {
        let should_rollback: ShouldRollbackFn<C> = <C as PartialEq>::ne;
        Self {
            prediction_mode: mode,
            correction: None,
            should_rollback: unsafe {
                std::mem::transmute::<for<'a, 'b> fn(&'a C, &'b C) -> bool, unsafe fn()>(
                    should_rollback,
                )
            },
            buffer_sync: ComponentRegistry::buffer_sync::<C>,
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
    Tick,
    &mut EntityWorldMut,
    &mut ReceiveEntityMap,
    &mut ConnectionEvents,
) -> Result<(), ComponentError>;

type RawBufferInsertFn = fn(
    &mut ComponentRegistry,
    &mut Reader,
    Tick,
    &mut EntityWorldMut,
    &mut ReceiveEntityMap,
    &mut ConnectionEvents,
) -> Result<(), ComponentError>;

type SyncFn = fn(&mut ComponentRegistry, confirmed: Entity, predicted: Entity, &World);

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
            if interpolation_data.interpolation_mode == ComponentSyncMode::Full
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

    pub(crate) fn register_component<C: Component + Message + Serialize + DeserializeOwned>(
        &mut self,
        world: &mut World,
    ) {
        self.register_component_custom_serde(world, SerializeFns::<C>::default());
    }

    pub(crate) fn register_component_custom_serde<C: Component + Message>(
        &mut self,
        world: &mut World,
        serialize_fns: SerializeFns<C>,
    ) {
        let component_kind = self.kind_map.add::<C>();
        let component_id = world.register_component::<C>();
        self.component_id_to_kind
            .insert(component_id, component_kind);
        self.kind_to_component_id
            .insert(component_kind, component_id);
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

        pub(crate) fn serialize<C: Message>(
            &self,
            component: &mut C,
            writer: &mut Writer,
            entity_map: &mut SendEntityMap,
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
            entity_map: &mut SendEntityMap,
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
        pub(crate) fn raw_deserialize<C: Message>(
            &self,
            reader: &mut Reader,
            entity_map: &mut ReceiveEntityMap,
        ) -> Result<C, ComponentError> {
            let kind = ComponentKind::of::<C>();
            let erased_fns = self
                .serialize_fns_map
                .get(&kind)
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
            self.raw_deserialize(reader, entity_map)
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
    use crate::client::prediction::predicted_history::PredictionHistory;
    use crate::client::prediction::resource::PredictionManager;
    use bevy::prelude::Entity;

    impl ComponentRegistry {
        pub(crate) fn predicted_component_ids(
            &self,
        ) -> impl Iterator<Item = ComponentId> + use<'_> {
            self.prediction_map
                .keys()
                .filter_map(|kind| self.kind_to_component_id.get(kind).copied())
        }

        pub(crate) fn set_prediction_mode<C: SyncComponent>(&mut self, mode: ComponentSyncMode) {
            let kind = ComponentKind::of::<C>();
            let default_equality_fn = <C as PartialEq>::eq;
            self.prediction_map
                .entry(kind)
                .or_insert_with(|| PredictionMetadata::default_from::<C>(mode));
        }

        pub(crate) fn set_should_rollback<C: SyncComponent + PartialEq>(
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

        pub(crate) fn set_linear_correction<C: SyncComponent + Linear + PartialEq>(&mut self) {
            self.set_correction(<C as Linear>::lerp);
        }

        pub(crate) fn set_correction<C: SyncComponent + PartialEq>(
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

        pub(crate) fn get_prediction_mode(
            &self,
            id: ComponentId,
        ) -> Result<ComponentSyncMode, ComponentError> {
            let kind = self
                .component_id_to_kind
                .get(&id)
                .ok_or(ComponentError::NotRegistered)?;
            Ok(self
                .prediction_map
                .get(kind)
                .map_or(ComponentSyncMode::None, |metadata| metadata.prediction_mode))
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
                .is_some_and(|metadata| metadata.correction.is_some())
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

        /// Clone the components from the confirmed entity to the predicted entity
        /// All the cloned components are inserted at once.
        pub(crate) fn batch_sync(
            &mut self,
            component_ids: &[ComponentId],
            confirmed: Entity,
            predicted: Entity,
            world: &mut World,
        ) {
            // clone each component to be synced into a temporary buffer
            component_ids.iter().for_each(|component_id| {
                let kind = self.component_id_to_kind.get(component_id).unwrap();
                let prediction_metadata = self
                    .prediction_map
                    .get(kind)
                    .expect("the component is not part of the protocol");
                (prediction_metadata.buffer_sync)(self, confirmed, predicted, world);
            });
            // insert all the components in the predicted entity
            let mut entity_world_mut = world.entity_mut(predicted);
            self.temp_write_buffer.batch_insert(&mut entity_world_mut);
        }

        /// Sync a component value from the confirmed entity to the predicted entity
        pub(crate) fn buffer_sync<C: SyncComponent>(
            &mut self,
            confirmed: Entity,
            predicted: Entity,
            world: &World,
        ) {
            let kind = ComponentKind::of::<C>();
            let prediction_metadata = self
                .prediction_map
                .get(&kind)
                .expect("the component is not part of the protocol");
            // for Full components, also insert a PredictionHistory component
            // no need to add any value to it because otherwise it would contain a value with the wrong tick
            // since we are running this outside of FixedUpdate
            if prediction_metadata.prediction_mode == ComponentSyncMode::Full {
                // if the predicted entity already had a PredictionHistory component (for example
                // if the entity was PreSpawned entity), we don't want to overwrite it.
                if world.get::<PredictionHistory<C>>(predicted).is_none() {
                    unsafe {
                        self.temp_write_buffer.buffer_insert_raw_ptrs(
                            PredictionHistory::<C>::default(),
                            world
                                .component_id::<PredictionHistory<C>>()
                                .expect("PredictionHistory not registered"),
                        )
                    };
                }
            }

            // TODO: add a test for this! For PreSpawned/PrePredicted we don't want to sync from Confirmed to Predicted
            // TODO: does this interact well with cases where the component is removed on the predicted entity?
            // if the predicted entity already has the component, we don't want to sync it:
            // - if the predicted entity is Predicted/PrePredicted/PreSpawned, we would be overwriting the predicted value, instead
            //   of letting the rollback systems work
            // - if the component is ComponentSyncMode::Once, we only need to sync it once
            // - if the component is ComponentSyncMode::Simple, every component update will be synced via a separate system
            if world.get::<C>(predicted).is_some() {
                return;
            }
            let value = world.get::<C>(confirmed).unwrap();
            let mut clone = value.clone();
            world
                .resource::<PredictionManager>()
                .map_entities(&mut clone, self)
                .unwrap();
            unsafe {
                self.temp_write_buffer
                    .buffer_insert_raw_ptrs(clone, world.component_id::<C>().unwrap())
            };
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
    use crate::prelude::{DeltaCompression, OverrideTargetComponent, ReplicateOnceComponent};
    use crate::serialize::reader::Reader;
    use crate::serialize::ToBytes;
    use crate::shared::replication::entity_map::ReceiveEntityMap;
    use bytes::Bytes;

    type EntityHashMap<K, V> = hashbrown::HashMap<K, V, EntityHash>;

    impl ComponentRegistry {
        pub(crate) fn direction(&self, kind: ComponentKind) -> Option<ChannelDirection> {
            self.replication_map
                .get(&kind)
                .map(|metadata| metadata.direction)
        }

        pub(crate) fn set_replication_fns<C: Component + PartialEq>(
            &mut self,
            world: &mut World,
            direction: ChannelDirection,
        ) {
            let kind = ComponentKind::of::<C>();
            let write: RawWriteFn = Self::write::<C>;
            let remove: RawRemoveFn = Self::remove::<C>;
            self.replication_map.insert(
                kind,
                ReplicationMetadata {
                    direction,
                    delta_compression_id: world.register_component::<DeltaCompression<C>>(),
                    replicate_once_id: world.register_component::<ReplicateOnceComponent<C>>(),
                    override_target_id: world.register_component::<OverrideTargetComponent<C>>(),
                    write,
                    buffer_insert_fn: Self::buffer_insert::<C>,
                    remove: Some(remove),
                },
            );
        }

        /// Insert a batch of components on the entity
        ///
        /// This method will insert all the components simultaneously.
        /// If any component already existed on the entity, it will be updated instead of inserted.
        pub(crate) fn batch_insert(
            &mut self,
            component_bytes: Vec<Bytes>,
            entity_world_mut: &mut EntityWorldMut,
            tick: Tick,
            entity_map: &mut ReceiveEntityMap,
            events: &mut ConnectionEvents,
        ) -> Result<(), ComponentError> {
            component_bytes.into_iter().try_for_each(|b| {
                // TODO: reuse a single reader that reads through the entire message ?
                let mut reader = Reader::from(b);
                let net_id =
                    ComponentNetId::from_bytes(&mut reader).map_err(SerializationError::from)?;
                let kind = self
                    .kind_map
                    .kind(net_id)
                    .ok_or(ComponentError::NotRegistered)?;
                let replication_metadata = self
                    .replication_map
                    .get(kind)
                    .ok_or(ComponentError::MissingReplicationFns)?;
                // buffer the component data into the temporary buffer so that
                // all components can be inserted at once
                (replication_metadata.buffer_insert_fn)(
                    self,
                    &mut reader,
                    tick,
                    entity_world_mut,
                    entity_map,
                    events,
                )?;
                Ok::<(), ComponentError>(())
            })?;

            // TODO: sort by component id for cache efficiency!
            //  maybe it's not needed because on the server side we iterate through archetypes in a deterministic order?
            // # Safety
            // - Each [`ComponentId`] is from the same world as [`EntityWorldMut`]
            // - Each [`OwningPtr`] is a valid reference to the type represented by [`ComponentId`]
            //   (the data is store in self.raw_bytes)

            trace!(?self.temp_write_buffer.component_ids, "Inserting components into entity");
            self.temp_write_buffer.batch_insert(entity_world_mut);
            Ok(())
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
            (replication_metadata.write)(self, reader, tick, entity_world_mut, entity_map, events)?;
            Ok(*kind)
        }

        /// Method that buffers a pointer to the component data that will be inserted
        /// in the entity inside `self.raw_bytes`
        pub(crate) fn buffer_insert<C: Component + PartialEq>(
            &mut self,
            reader: &mut Reader,
            tick: Tick,
            entity_world_mut: &mut EntityWorldMut,
            entity_map: &mut ReceiveEntityMap,
            events: &mut ConnectionEvents,
        ) -> Result<(), ComponentError> {
            let kind = ComponentKind::of::<C>();
            let component_id = self
                .kind_to_component_id
                .get(&kind)
                .ok_or(ComponentError::NotRegistered)?;
            let component = self.raw_deserialize::<C>(reader, entity_map)?;
            let entity = entity_world_mut.id();
            debug!("Insert component {} to entity", std::any::type_name::<C>());

            // if the component is already on the entity, no need to insert
            if let Some(mut c) = entity_world_mut.get_mut::<C>() {
                // TODO: when can we be in this situation? on authority change?
                // only apply the update if the component is different, to not trigger change detection
                if c.as_ref() != &component {
                    #[cfg(feature = "metrics")]
                    {
                        metrics::counter!("replication::receive::component::update").increment(1);
                        metrics::counter!(format!(
                            "replication::receive::component::{}::update",
                            std::any::type_name::<C>()
                        ))
                        .increment(1);
                    }
                    events.push_update_component(entity, kind, tick);
                    *c = component;
                }
            } else {
                // TODO: add safety comment
                unsafe {
                    self.temp_write_buffer
                        .buffer_insert_raw_ptrs::<C>(component, *component_id)
                };
                // TODO: should we send the event based on on the message type (Insert/Update) or based on whether the component was actually inserted?
                #[cfg(feature = "metrics")]
                {
                    metrics::counter!("replication::receive::component::insert").increment(1);
                    metrics::counter!(format!(
                        "replication::receive::component::{}::insert",
                        std::any::type_name::<C>()
                    ))
                    .increment(1);
                }
                events.push_insert_component(entity, kind, tick);
            }
            Ok(())
        }

        pub(crate) fn write<C: Component + PartialEq>(
            &self,
            reader: &mut Reader,
            tick: Tick,
            entity_world_mut: &mut EntityWorldMut,
            entity_map: &mut ReceiveEntityMap,
            events: &mut ConnectionEvents,
        ) -> Result<(), ComponentError> {
            debug!("Writing component {} to entity", std::any::type_name::<C>());
            let kind = ComponentKind::of::<C>();
            let component = self.raw_deserialize::<C>(reader, entity_map)?;
            let entity = entity_world_mut.id();
            // TODO: should we send the event based on on the message type (Insert/Update) or based on whether the component was actually inserted?
            if let Some(mut c) = entity_world_mut.get_mut::<C>() {
                // only apply the update if the component is different, to not trigger change detection
                if c.as_ref() != &component {
                    #[cfg(feature = "metrics")]
                    {
                        metrics::counter!("replication::receive::component::update").increment(1);
                        metrics::counter!(format!(
                            "replication::receive::component::{}::update",
                            std::any::type_name::<C>()
                        ))
                        .increment(1);
                    }
                    events.push_update_component(entity, kind, tick);
                    *c = component;
                }
            } else {
                #[cfg(feature = "metrics")]
                {
                    metrics::counter!("replication::receive::component::insert").increment(1);
                    metrics::counter!(format!(
                        "replication::receive::component::{}::insert",
                        std::any::type_name::<C>()
                    ))
                    .increment(1);
                }
                events.push_insert_component(entity, kind, tick);
                entity_world_mut.insert(component);
            }
            Ok(())
        }

        pub(crate) fn raw_remove(
            &self,
            net_id: ComponentNetId,
            entity_world_mut: &mut EntityWorldMut,
            tick: Tick,
            events: &mut ConnectionEvents,
        ) {
            let kind = self.kind_map.kind(net_id).expect("unknown component kind");
            let replication_metadata = self
                .replication_map
                .get(kind)
                .expect("the component is not part of the protocol");
            let f = replication_metadata
                .remove
                .expect("the component does not have a remove function");
            events.push_remove_component(entity_world_mut.id(), *kind, tick);
            f(self, entity_world_mut);
        }

        pub(crate) fn remove<C: Component>(&self, entity_world_mut: &mut EntityWorldMut) {
            #[cfg(feature = "metrics")]
            {
                metrics::counter!("replication::receive::component::remove").increment(1);
                metrics::counter!(format!(
                    "replication::receive::component::{}::remove",
                    std::any::type_name::<C>()
                ))
                .increment(1);
            }
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
        pub(crate) fn set_delta_compression<C: Component + PartialEq + Diffable>(
            &mut self,
            world: &mut World,
        ) where
            C::Delta: Serialize + DeserializeOwned,
        {
            let kind = ComponentKind::of::<C>();
            let delta_kind = ComponentKind::of::<DeltaMessage<C::Delta>>();
            // add the delta as a message
            self.register_component::<DeltaMessage<C::Delta>>(world);
            // add delta-related type-erased functions
            self.delta_fns_map.insert(kind, ErasedDeltaFns::new::<C>());
            // add write/remove functions associated with the delta component's net_id
            // (since the serialized message will contain the delta component's net_id)
            // update the write function to use the delta compression logic
            let write: RawWriteFn = Self::write_delta::<C>;
            self.replication_map.insert(
                delta_kind,
                ReplicationMetadata {
                    // Note: the direction should always exist; adding unwrap_or for unit tests
                    direction: self
                        .replication_map
                        .get(&kind)
                        .map(|m| m.direction)
                        .unwrap_or(ChannelDirection::Bidirectional),
                    // NOTE: we set these to 0 because they are never used for the DeltaMessage component
                    delta_compression_id: ComponentId::new(0),
                    replicate_once_id: ComponentId::new(0),
                    override_target_id: ComponentId::new(0),
                    write,
                    buffer_insert_fn: Self::buffer_insert_delta::<C>,
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
            entity_map: &mut SendEntityMap,
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
            entity_map: &mut SendEntityMap,
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
            tick: Tick,
            entity_world_mut: &mut EntityWorldMut,
            entity_map: &mut ReceiveEntityMap,
            events: &mut ConnectionEvents,
        ) -> Result<(), ComponentError> {
            trace!(
                "Writing component delta {} to entity",
                std::any::type_name::<C>()
            );
            let kind = ComponentKind::of::<C>();
            let delta_net_id = self.net_id::<DeltaMessage<C::Delta>>();
            let delta = self.raw_deserialize::<DeltaMessage<C::Delta>>(reader, entity_map)?;
            let entity = entity_world_mut.id();
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
                    // (since we now know that the sender has received an ack for previous_tick, otherwise it wouldn't
                    // have sent a diff based on the previous_tick)
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

                    // TODO: should we send the event based on the message type (Insert/Update) or based on whether the component was actually inserted?
                    events.push_update_component(entity, kind, tick);
                }
                DeltaType::FromBase => {
                    let mut new_value = C::base_value();
                    new_value.apply_diff(&delta.delta);
                    let value = new_value.clone();
                    if let Some(mut c) = entity_world_mut.get_mut::<C>() {
                        // only apply the update if the component is different, to not trigger change detection
                        if c.as_ref() != &new_value {
                            *c = new_value;
                            events.push_update_component(entity, kind, tick);
                        }
                    } else {
                        entity_world_mut.insert(new_value);
                        events.push_insert_component(entity, kind, tick);
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

        /// Insert a component delta into the entity.
        /// If the component is not present on the entity, we put it in a temporary buffer
        /// so that all components can be inserted at once
        pub(crate) fn buffer_insert_delta<C: Component + PartialEq + Diffable>(
            &mut self,
            reader: &mut Reader,
            tick: Tick,
            entity_world_mut: &mut EntityWorldMut,
            entity_map: &mut ReceiveEntityMap,
            events: &mut ConnectionEvents,
        ) -> Result<(), ComponentError> {
            let kind = ComponentKind::of::<C>();
            let component_id = self
                .kind_to_component_id
                .get(&kind)
                .ok_or(ComponentError::NotRegistered)?;
            trace!(
                ?kind,
                ?component_id,
                "Writing component delta {} to entity",
                std::any::type_name::<C>()
            );
            let delta = self.raw_deserialize::<DeltaMessage<C::Delta>>(reader, entity_map)?;
            let entity = entity_world_mut.id();
            match delta.delta_type {
                DeltaType::Normal { previous_tick } => {
                    unreachable!("buffer_insert_delta should only be called for FromBase deltas since the component is being inserted");
                }
                DeltaType::FromBase => {
                    let mut new_value = C::base_value();
                    new_value.apply_diff(&delta.delta);
                    // clone the value so that we can insert it in the history
                    let cloned_value = new_value.clone();

                    // if the component is on the entity, no need to insert
                    if let Some(mut c) = entity_world_mut.get_mut::<C>() {
                        // only apply the update if the component is different, to not trigger change detection
                        if c.as_ref() != &new_value {
                            *c = new_value;
                            events.push_update_component(entity, kind, tick);
                        }
                    } else {
                        // TODO: add safety comment
                        // use the component id of C, not DeltaMessage<C>
                        unsafe {
                            self.temp_write_buffer
                                .buffer_insert_raw_ptrs::<C>(new_value, *component_id)
                        };
                        events.push_insert_component(entity, kind, tick);
                    }
                    // store the component value in the delta component history, so that we can compute
                    // diffs from it
                    if let Some(mut history) =
                        entity_world_mut.get_mut::<DeltaComponentHistory<C>>()
                    {
                        history.buffer.insert(tick, cloned_value);
                    } else {
                        // create a DeltaComponentHistory and insert the value
                        let mut history = DeltaComponentHistory::default();
                        history.buffer.insert(tick, cloned_value);
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
    fn add_resource_rollback<R: Resource + Clone>(&mut self);

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
                    debug!("register component {}", std::any::type_name::<C>());
                    registry.register_component::<C>(world);
                    registry.set_replication_fns::<C>(world, direction);
                }
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
                    registry.register_component_custom_serde::<C>(world, serialize_fns);
                }
                registry.set_replication_fns::<C>(world, direction);
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
    fn add_resource_rollback<R: Resource + Clone>(&mut self) {
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
        self.world_mut()
            .resource_scope(|world, mut registry: Mut<ComponentRegistry>| {
                registry.set_delta_compression::<C>(world);
            })
    }
}

/// [`ComponentKind`] is an internal wrapper around the type of the component
#[derive(Debug, Eq, Hash, Copy, Clone, PartialEq, Reflect)]
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
    use crate::shared::replication::entity_map::SendEntityMap;
    use crate::tests::protocol::*;
    use bevy::prelude::{Commands, OnAdd, OnInsert, Query, Trigger};

    #[test]
    fn test_custom_serde() {
        let mut world = World::new();
        let mut registry = ComponentRegistry::default();
        registry.register_component_custom_serde::<ComponentSyncModeSimple>(
            &mut world,
            SerializeFns {
                serialize: serialize_component2,
                deserialize: deserialize_component2,
            },
        );
        let mut component = ComponentSyncModeSimple(1.0);
        let mut writer = Writer::default();
        registry
            .serialize(&mut component, &mut writer, &mut SendEntityMap::default())
            .unwrap();
        let data = writer.to_bytes();

        let mut reader = Reader::from(data);
        let read = registry
            .deserialize(&mut reader, &mut ReceiveEntityMap::default())
            .unwrap();
        assert_eq!(component, read);
    }

    #[derive(Debug, Default, Clone, PartialEq, TypePath, Resource)]
    struct Buffer(TempWriteBuffer);

    /// Make sure that the temporary buffer works properly even if it's being used recursively
    /// because of observers
    #[test]
    fn test_recursive_temp_write_buffer() {
        let mut world = World::new();
        world.init_resource::<Buffer>();

        world.add_observer(
            |trigger: Trigger<OnAdd, ComponentSyncModeFull>, mut commands: Commands| {
                let entity = trigger.entity();
                commands.queue(move |world: &mut World| {
                    let component_id_once = world.register_component::<ComponentSyncModeOnce>();
                    let component_id_simple = world.register_component::<ComponentSyncModeSimple>();
                    let unsafe_world = world.as_unsafe_world_cell();
                    let mut buffer = unsafe { unsafe_world.get_resource_mut::<Buffer>() }.unwrap();
                    unsafe {
                        buffer.0.buffer_insert_raw_ptrs::<_>(
                            ComponentSyncModeOnce(1.0),
                            component_id_once,
                        )
                    }
                    unsafe {
                        buffer.0.buffer_insert_raw_ptrs::<_>(
                            ComponentSyncModeSimple(1.0),
                            component_id_simple,
                        )
                    }
                    // we insert both Once and Simple into the entity
                    let mut entity_world_mut =
                        unsafe { unsafe_world.world_mut() }.entity_mut(entity);
                    buffer.0.batch_insert(&mut entity_world_mut);
                })
            },
        );
        world.add_observer(
            |trigger: Trigger<OnAdd, ComponentSyncModeOnce>, mut commands: Commands| {
                let entity = trigger.entity();
                commands.queue(move |world: &mut World| {
                    let component_id = world.register_component::<ComponentSyncModeSimple>();
                    let unsafe_world = world.as_unsafe_world_cell();
                    let mut buffer = unsafe { unsafe_world.get_resource_mut::<Buffer>() }.unwrap();
                    unsafe {
                        buffer
                            .0
                            .buffer_insert_raw_ptrs::<_>(ComponentSyncModeSimple(1.0), component_id)
                    }
                    // we insert only Simple into the entity.
                    // we should NOT also be inserting the components that were previously in the buffer (Once) a second time
                    let mut entity_world_mut =
                        unsafe { unsafe_world.world_mut() }.entity_mut(entity);
                    buffer.0.batch_insert(&mut entity_world_mut);
                })
            },
        );
        world.add_observer(
            |trigger: Trigger<OnInsert, ComponentSyncModeSimple>,
             mut query: Query<&mut ComponentSyncModeFull>| {
                if let Ok(mut comp) = query.get_mut(trigger.entity()) {
                    comp.0 += 1.0;
                }
            },
        );
        world.spawn(ComponentSyncModeFull(0.0));
        world.flush();

        // make sure that the ComponentSyncModeSimple was only inserted twice, not three times
        assert_eq!(
            world.query::<&ComponentSyncModeFull>().single(&world).0,
            2.0
        );
    }
}
