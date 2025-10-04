use crate::components::Confirmed;
use crate::delta::Diffable;
use crate::prelude::{ComponentReplicationConfig, ComponentReplicationOverrides};
use crate::registry::delta::ErasedDeltaFns;
use crate::registry::replication::{GetWriteFns, ReplicationMetadata};
use crate::registry::{ComponentError, ComponentKind, ComponentNetId};
use bevy_app::App;
use bevy_ecs::{
    component::{Component, ComponentId, Mutable},
    entity::MapEntities,
    resource::Resource,
    world::{Mut, World},
};
use bevy_platform::collections::HashMap;
use bevy_ptr::{Ptr, PtrMut};
use bevy_reflect::TypePath;
use bevy_transform::components::Transform;
use bevy_utils::prelude::DebugName;
use lightyear_core::network::NetId;
use lightyear_messages::Message;
use lightyear_serde::entity_map::{EntityMap, ReceiveEntityMap, SendEntityMap};
use lightyear_serde::reader::Reader;
use lightyear_serde::registry::{
    ContextDeserializeFn, ContextDeserializeFns, ContextSerializeFn, ContextSerializeFns,
    DeserializeFn, ErasedSerializeFns, SerializeFn, SerializeFns,
};
use lightyear_serde::writer::Writer;
use lightyear_serde::{SerializationError, ToBytes};
use lightyear_utils::registry::{RegistryHash, RegistryHasher, TypeMapper};
use serde::de::DeserializeOwned;
use serde::ser::Serialize;
#[cfg(feature = "trace")]
use tracing::{Level, instrument};
#[allow(unused_imports)]
use tracing::{debug, info, trace};

/// Function used to interpolate from one component state (`start`) to another (`other`)
/// t goes from 0.0 (`start`) to 1.0 (`other`)
pub type LerpFn<C> = fn(start: C, other: C, t: f32) -> C;

/// A [`Resource`] that will keep track of all the [`Components`](Component) that can be replicated.
///
///
/// ### Adding Components
///
/// You register components by calling the [`register_component`](AppComponentExt::register_component) method directly on the App.
///
/// By default, a component needs to implement `Serialize` and `Deserialize`, but you can also provide your own
/// serialization functions by using the [`register_component_custom_serde`](AppComponentExt::register_component_custom_serde) method.
///
/// ```rust
/// # use bevy_app::App;
/// # use bevy_ecs::component::Component;
/// # use serde::{Deserialize, Serialize};
/// # use lightyear_replication::registry::registry::AppComponentExt;
///
/// #[derive(Component, PartialEq, Serialize, Deserialize)]
/// struct MyComponent;
///
/// fn add_components(app: &mut App) {
///   app.register_component::<MyComponent>();
/// }
/// ```
///
/// ### Customizing Component behaviour
///
/// There are some cases where you might want to define additional behaviour for a component.
///
/// #### Entity Mapping
/// If the component contains any [`Entity`](bevy_ecs::prelude::Entity), you need to specify how those entities
/// will be mapped from the remote world to the local world.
///
/// Provided that your type implements [`MapEntities`], you can extend the protocol to support this behaviour, by
/// calling the [`add_map_entities`](ComponentRegistration::add_map_entities) method.
///
/// #### Prediction
/// When client-prediction is enabled, a predicted entity is one that has the [`Predicted`](lightyear_core::prelude::Predicted) component.
///
/// You have to specify which components are predicted by calling the `add_prediction` method.
///
/// #### Correction
/// When client-prediction is enabled, there might be cases where there is a mismatch between the state of the Predicted entity
/// and the state of the Confirmed entity. In this case, we rollback by snapping the Predicted entity to the Confirmed entity and replaying the last few frames.
///
/// However, rollbacks that do an instant update can be visually jarring, so we provide the option to smooth the rollback process over a few frames.
/// You can do this by calling the `add_correction_fn` method.
///
/// If your component implements the `Ease` trait, you can use the `add_linear_correction_fn` method,
/// which provides linear interpolation.
///
/// #### Interpolation
/// Similarly to client-prediction, an interpolated entity has the [`Interpolated`](lightyear_core::prelude::Interpolated) component.
///
/// Interpolated componnets are added by calling the `add_interpolation` method and will interpolate between two
/// consecutive replicated updates.
///
/// You will also need to provide an interpolation function that will be used to interpolate between two states.
/// If your component implements the `Ease` trait, you can use the `add_linear_interpolation_fn` method,
/// which means that we will interpolate using linear interpolation.
///
/// You can also use your own interpolation function by using the `add_interpolation_fn` method.
///
/// ```rust,ignore
/// use bevy_app::App;
/// use bevy_ecs::component::Component;
/// use serde::{Deserialize, Serialize};
/// use lightyear_replication::prelude::AppComponentExt;
///
/// #[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
/// struct MyComponent(f32);
///
/// fn my_lerp_fn(start: MyComponent, other: MyComponent, t: f32) -> MyComponent {
///    MyComponent(start.0 * (1.0 - t) + other.0 * t)
/// }
///
/// fn add_messages(app: &mut App) {
///   app.register_component::<MyComponent>()
///       .add_prediction(PredictionMode::Full)
///       .add_interpolation(InterpolationMode::Full)
///       .add_interpolation_fn(my_lerp_fn);
/// }
/// ```
#[derive(Debug, Default, Clone, Resource, TypePath)]
pub struct ComponentRegistry {
    pub component_id_to_kind: HashMap<ComponentId, ComponentKind>,
    pub component_metadata_map: HashMap<ComponentKind, ComponentMetadata>,
    pub kind_map: TypeMapper<ComponentKind>,
    hasher: RegistryHasher,
}

#[derive(Debug, Clone)]
pub struct ComponentMetadata {
    pub confirmed_component_id: ComponentId,
    pub component_id: ComponentId,
    pub replication: Option<ReplicationMetadata>,
    pub serialization: Option<ErasedSerializeFns>,
    pub(crate) delta: Option<ErasedDeltaFns>,
    #[cfg(feature = "deterministic")]
    pub deterministic: Option<super::deterministic::DeterministicFns>,
}

impl ComponentRegistry {
    pub fn net_id<C: 'static>(&self) -> ComponentNetId {
        self.kind_map
            .net_id(&ComponentKind::of::<C>())
            .copied()
            .unwrap_or_else(|| {
                panic!(
                    "Component {} is not registered",
                    DebugName::type_name::<C>()
                )
            })
    }
    pub fn get_net_id<C: 'static>(&self) -> Option<ComponentNetId> {
        self.kind_map.net_id(&ComponentKind::of::<C>()).copied()
    }

    pub fn is_registered<C: 'static>(&self) -> bool {
        self.kind_map.net_id(&ComponentKind::of::<C>()).is_some()
    }

    pub fn register_component<C: Component + Serialize + DeserializeOwned>(
        &mut self,
        world: &mut World,
    ) {
        self.register_component_custom_serde(world, SerializeFns::<C>::default());
    }

    pub fn register_component_custom_serde<C: Component>(
        &mut self,
        world: &mut World,
        serialize_fns: SerializeFns<C>,
    ) {
        let component_kind = self.kind_map.add::<C>();
        let component_id = world.register_component::<C>();
        let confirmed_component_id = world.register_component::<Confirmed<C>>();
        self.component_id_to_kind
            .insert(component_id, component_kind);
        self.component_metadata_map
            .entry(component_kind)
            .or_insert(ComponentMetadata {
                confirmed_component_id,
                component_id,
                replication: None,
                serialization: None,
                delta: None,
                #[cfg(feature = "deterministic")]
                deterministic: None,
            })
            .serialization = Some(ErasedSerializeFns::new::<
            SendEntityMap,
            ReceiveEntityMap,
            C,
            C,
        >(
            ContextSerializeFns::new(serialize_fns.serialize),
            ContextDeserializeFns::new(serialize_fns.deserialize),
        ));
    }

    pub fn finish(&mut self) -> RegistryHash {
        self.hasher.finish()
    }
}

fn mapped_context_serialize<M: MapEntities + Clone>(
    mapper: &mut SendEntityMap,
    message: &M,
    writer: &mut Writer,
    serialize_fn: SerializeFn<M>,
) -> Result<(), SerializationError> {
    let mut message = message.clone();
    trace!(
        "mapped_context_serialize: {:?}. Mapper: {:?}",
        DebugName::type_name::<M>(),
        mapper
    );
    message.map_entities(mapper);
    serialize_fn(&message, writer)
}

fn mapped_context_deserialize<M: MapEntities>(
    mapper: &mut ReceiveEntityMap,
    reader: &mut Reader,
    deserialize_fn: DeserializeFn<M>,
) -> Result<M, SerializationError> {
    let mut message = deserialize_fn(reader)?;
    message.map_entities(mapper);
    Ok(message)
}

fn component_map_entities<M: Component>(component: PtrMut, mapper: &mut EntityMap) {
    // SAFETY: the caller must ensure that the PtrMut corresponds to type M
    let component = unsafe { component.deref_mut::<M>() };
    Component::map_entities(component, mapper);
}

/// Serialize using the Component's `MapEntities` implementation to map entities before serializing
fn component_mapped_context_serialize<M: Component + Clone>(
    mapper: &mut SendEntityMap,
    message: &M,
    writer: &mut Writer,
    serialize_fn: SerializeFn<M>,
) -> Result<(), SerializationError> {
    let mut message = message.clone();
    Component::map_entities(&mut message, mapper);
    serialize_fn(&message, writer)
}

fn component_mapped_context_deserialize<M: Component>(
    mapper: &mut ReceiveEntityMap,
    reader: &mut Reader,
    deserialize_fn: DeserializeFn<M>,
) -> Result<M, SerializationError> {
    let mut message = deserialize_fn(reader)?;
    Component::map_entities(&mut message, mapper);
    Ok(message)
}

impl ComponentRegistry {
    pub(crate) fn try_add_map_entities<C: Clone + MapEntities + 'static>(&mut self) {
        let kind = ComponentKind::of::<C>();
        if let Some(metadata) = self.component_metadata_map.get_mut(&kind)
            && let Some(serialization) = metadata.serialization.as_mut()
        {
            serialization.add_map_entities::<C>();
        }
    }

    pub(crate) fn add_map_entities<C: Clone + MapEntities + 'static>(&mut self) {
        let kind = ComponentKind::of::<C>();
        let metadata = self
            .component_metadata_map
            .get_mut(&kind)
            .unwrap_or_else(|| {
                panic!(
                    "Component {} is not part of the protocol",
                    DebugName::type_name::<C>()
                )
            });
        let erased_fns = metadata.serialization.as_mut().unwrap();
        erased_fns.add_map_entities::<C>();
        let context_serialize: ContextSerializeFn<SendEntityMap, C, C> =
            mapped_context_serialize::<C>;
        let context_deserialize: ContextDeserializeFn<ReceiveEntityMap, C, C> =
            mapped_context_deserialize::<C>;
        erased_fns.context_serialize = unsafe { core::mem::transmute(context_serialize) };
        erased_fns.context_deserialize = unsafe { core::mem::transmute(context_deserialize) };
    }

    // Function for advanced users that is equivalent to `add_map_entities` but uses the Component::map_entities function
    pub(crate) fn add_component_map_entities<C: Clone + Component + 'static>(&mut self) {
        let kind = ComponentKind::of::<C>();
        let metadata = self
            .component_metadata_map
            .get_mut(&kind)
            .unwrap_or_else(|| {
                panic!(
                    "Component {} is not part of the protocol",
                    DebugName::type_name::<C>()
                )
            });
        let erased_fns = metadata.serialization.as_mut().unwrap();
        erased_fns.add_map_entities_with::<C>(component_map_entities::<C>);
        let context_serialize: ContextSerializeFn<SendEntityMap, C, C> =
            component_mapped_context_serialize::<C>;
        let context_deserialize: ContextDeserializeFn<ReceiveEntityMap, C, C> =
            component_mapped_context_deserialize::<C>;
        erased_fns.context_serialize = unsafe { core::mem::transmute(context_serialize) };
        erased_fns.context_deserialize = unsafe { core::mem::transmute(context_deserialize) };
    }

    pub(crate) fn serialize<C: 'static>(
        &self,
        component: &C,
        writer: &mut Writer,
        entity_map: &mut SendEntityMap,
    ) -> Result<(), ComponentError> {
        self.erased_serialize(
            Ptr::from(component),
            writer,
            ComponentKind::of::<C>(),
            entity_map,
        )
    }

    #[cfg_attr(feature = "trace", instrument(level = Level::INFO, skip_all))]
    /// SAFETY: the Ptr must correspond to the correct ComponentKind
    pub(crate) fn erased_serialize(
        &self,
        component: Ptr,
        writer: &mut Writer,
        kind: ComponentKind,
        entity_map: &mut SendEntityMap,
    ) -> Result<(), ComponentError> {
        let metadata = self
            .component_metadata_map
            .get(&kind)
            .ok_or(ComponentError::MissingSerializationFns)?;
        let erased_fns = metadata
            .serialization
            .as_ref()
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
        entity_map: &mut ReceiveEntityMap,
    ) -> Result<C, ComponentError> {
        let kind = ComponentKind::of::<C>();
        let metadata = self
            .component_metadata_map
            .get(&kind)
            .ok_or(ComponentError::MissingSerializationFns)?;
        let erased_fns = metadata
            .serialization
            .as_ref()
            .ok_or(ComponentError::MissingSerializationFns)?;
        // SAFETY: the ErasedFns corresponds to type C
        unsafe { erased_fns.deserialize::<_, C, C>(reader, entity_map) }.map_err(Into::into)
    }

    pub(crate) fn deserialize<C: Component>(
        &self,
        reader: &mut Reader,
        entity_map: &mut ReceiveEntityMap,
    ) -> Result<C, ComponentError> {
        let _ = NetId::from_bytes(reader)?;
        self.raw_deserialize(reader, entity_map)
    }

    pub fn map_entities<C: 'static>(
        &self,
        component: &mut C,
        entity_map: &mut EntityMap,
    ) -> Result<(), ComponentError> {
        let kind = ComponentKind::of::<C>();
        let metadata = self
            .component_metadata_map
            .get(&kind)
            .ok_or(ComponentError::MissingSerializationFns)?;
        let erased_fns = metadata
            .serialization
            .as_ref()
            .ok_or(ComponentError::MissingSerializationFns)?;
        erased_fns.map_entities(component, entity_map);
        Ok(())
    }
}

/// Add a component to the list of components that can be sent
pub trait AppComponentExt {
    /// Registers the component in the Registry
    /// This component can now be sent over the network.
    fn register_component<C: Component<Mutability: GetWriteFns<C>> + Serialize + DeserializeOwned>(
        &mut self,
    ) -> ComponentRegistration<'_, C>;

    /// Registers the component in the Registry: this component can now be sent over the network.
    ///
    /// You need to provide your own [`SerializeFns`]
    fn register_component_custom_serde<C: Component<Mutability: GetWriteFns<C>>>(
        &mut self,
        serialize_fns: SerializeFns<C>,
    ) -> ComponentRegistration<'_, C>;

    /// Returns a ComponentRegistration for a component that is not networked.
    ///
    /// This can be useful for components that are not networked but that you still need
    /// to sync to predicted or interpolated entities; or for which you need to enable
    /// rollback.
    fn non_networked_component<C: Component<Mutability: GetWriteFns<C>>>(
        &mut self,
    ) -> ComponentRegistration<'_, C>;
}

impl AppComponentExt for App {
    fn register_component<
        C: Component<Mutability: GetWriteFns<C>> + Serialize + DeserializeOwned,
    >(
        &mut self,
    ) -> ComponentRegistration<'_, C> {
        self.register_component_custom_serde(SerializeFns::<C>::default())
    }

    fn register_component_custom_serde<C: Component<Mutability: GetWriteFns<C>>>(
        &mut self,
        serialize_fns: SerializeFns<C>,
    ) -> ComponentRegistration<'_, C> {
        if self
            .world_mut()
            .get_resource_mut::<ComponentRegistry>()
            .is_none()
        {
            self.world_mut().init_resource::<ComponentRegistry>();
        }
        self.world_mut()
            .resource_scope(|world, mut registry: Mut<ComponentRegistry>| {
                if !registry.is_registered::<C>() {
                    registry.register_component_custom_serde::<C>(world, serialize_fns);
                }
                debug!("register component {}", DebugName::type_name::<C>());
            });
        ComponentRegistration {
            app: self,
            _phantom: core::marker::PhantomData,
        }
        // NOTE: apparently this is important; can't remove!
        .with_replication_config(ComponentReplicationConfig::default())
    }

    fn non_networked_component<C: Component<Mutability: GetWriteFns<C>>>(
        &mut self,
    ) -> ComponentRegistration<'_, C> {
        ComponentRegistration {
            app: self,
            _phantom: core::marker::PhantomData,
        }
    }
}

pub struct ComponentRegistration<'a, C> {
    pub app: &'a mut App,
    _phantom: core::marker::PhantomData<C>,
}

impl<C> ComponentRegistration<'_, C> {
    pub fn new(app: &mut App) -> ComponentRegistration<'_, C> {
        ComponentRegistration {
            app,
            _phantom: core::marker::PhantomData,
        }
    }

    /// Specify that the component contains entities which should be mapped from the remote world to the local world
    /// upon deserialization using the component's [`MapEntities`] implementation.
    pub fn add_map_entities(self) -> Self
    where
        C: Clone + MapEntities + 'static,
    {
        let mut registry = self.app.world_mut().resource_mut::<ComponentRegistry>();
        registry.add_map_entities::<C>();
        self
    }

    /// Similar to `add_map_entities`, but uses the `Component::map_entities` function instead of `MapEntities::map_entities`
    pub fn add_component_map_entities(self) -> Self
    where
        C: Clone + Component + 'static,
    {
        let mut registry = self.app.world_mut().resource_mut::<ComponentRegistry>();
        registry.add_component_map_entities::<C>();
        self
    }

    pub fn with_replication_config(self, config: ComponentReplicationConfig) -> Self
    where
        C: Component<Mutability: GetWriteFns<C>>,
    {
        let overrides_component_id = self
            .app
            .world_mut()
            .register_component::<ComponentReplicationOverrides<C>>();
        let mut registry = self.app.world_mut().resource_mut::<ComponentRegistry>();
        let kind = ComponentKind::of::<C>();
        let metadata = registry.component_metadata_map.get_mut(&kind).unwrap_or_else(|| {
            panic!(
                "Component {} is not part of the protocol, did you forget to call register_component?",
                DebugName::type_name::<C>()
            );
        });
        metadata.replication = Some(ReplicationMetadata::default_fns::<C>(
            config,
            overrides_component_id,
        ));
        self
    }

    /// Enable delta compression when serializing this component
    pub fn add_delta_compression<Delta>(self) -> Self
    where
        C: Component<Mutability = Mutable> + PartialEq + Diffable<Delta>,
        Delta: Serialize + DeserializeOwned + Message,
    {
        self.app
            .world_mut()
            .resource_scope(|world, mut registry: Mut<ComponentRegistry>| {
                registry.set_delta_compression::<C, Delta>(world);
            });
        self
    }
}

pub struct TransformLinearInterpolation;

impl TransformLinearInterpolation {
    pub fn lerp(start: Transform, other: Transform, t: f32) -> Transform {
        let translation = start.translation * (1.0 - t) + other.translation * t;
        let rotation = start.rotation.slerp(other.rotation, t);
        let scale = start.scale * (1.0 - t) + other.scale * t;
        let res = Transform {
            translation,
            rotation,
            scale,
        };
        trace!(
            "position lerp: start: {:?} end: {:?} t: {} res: {:?}",
            start, other, t, res
        );
        res
    }
}

#[cfg(test)]
mod tests {
    // use super::*;
    // use crate::serialize::writer::Writer;
    // use crate::shared::replication::entity_map::SendEntityMap;
    // use crate::tests::protocol::*;
    //
    // #[test]
    // fn test_custom_serde() {
    //     let mut world = World::new();
    //     let mut registry = ComponentRegistry::default();
    //     registry.register_component_custom_serde::<ComponentSyncModeSimple>(
    //         &mut world,
    //         SerializeFns {
    //             serialize: serialize_component2,
    //             deserialize: deserialize_component2,
    //         },
    //     );
    //     let mut component = ComponentSyncModeSimple(1.0);
    //     let mut writer = Writer::default();
    //     registry
    //         .serialize(&mut component, &mut writer, &mut SendEntityMap::default())
    //         .unwrap();
    //     let data = writer.to_bytes();
    //
    //     let mut reader = Reader::from(data);
    //     let read = registry
    //         .deserialize(&mut reader, &mut ReceiveEntityMap::default())
    //         .unwrap();
    //     assert_eq!(component, read);
    // }
}
