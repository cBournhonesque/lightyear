use crate::components::ComponentReplicationConfig;
use crate::delta::Diffable;
use crate::prelude::ComponentReplicationOverrides;
use crate::registry::delta::ErasedDeltaFns;
use crate::registry::replication::{GetWriteFns, ReplicationMetadata};
use crate::registry::{ComponentError, ComponentKind, ComponentNetId};
use bevy::app::App;
use bevy::ecs::change_detection::Mut;
use bevy::ecs::component::{Component, ComponentId, Mutable};
use bevy::ecs::entity::MapEntities;
use bevy::platform::collections::HashMap;
use bevy::prelude::{Resource, Transform, TypePath, World};
use bevy::ptr::Ptr;
use lightyear_core::network::NetId;
use lightyear_serde::entity_map::{EntityMap, ReceiveEntityMap, SendEntityMap};
use lightyear_serde::reader::Reader;
use lightyear_serde::registry::{
    ContextDeserializeFn, ContextDeserializeFns, ContextSerializeFn, ContextSerializeFns,
    DeserializeFn, ErasedSerializeFns, SerializeFn, SerializeFns,
};
use lightyear_serde::writer::Writer;
use lightyear_serde::{SerializationError, ToBytes};
use lightyear_utils::registry::TypeMapper;
use serde::de::DeserializeOwned;
use serde::ser::Serialize;
use tracing::{debug, trace};

/// Function used to interpolate from one component state (`start`) to another (`other`)
/// t goes from 0.0 (`start`) to 1.0 (`other`)
pub type LerpFn<C> = fn(start: C, other: C, t: f32) -> C;

/// A [`Resource`] that will keep track of all the [`Components`](Component) that can be replicated.
///
///
/// ### Adding Components
///
/// You register components by calling the [`register_component`](AppComponentExt::register_component) method directly on the App.
/// You can provide a [`NetworkDirection`] to specify if the component should be sent from the client to the server, from the server to the client, or both.
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
///   app.register_component::<MyComponent>();
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
/// use serde::{Deserialize, Serialize};
/// use lightyear::prelude::*;
/// use lightyear::prelude::client::*;
///
/// #[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
/// struct MyComponent(f32);
///
/// fn my_lerp_fn(start: MyComponent, other: MyComponent, t: f32) -> MyComponent {
///    MyComponent(start.0 * (1.0 - t) + other.0 * t)
/// }
///
///
/// fn add_messages(app: &mut App) {
///   app.register_component::<MyComponent>()
///       .add_prediction(PredictionMode::Full)
///       .add_interpolation(InterpolationMode::Full)
///       .add_interpolation_fn(my_lerp_fn);
/// }
/// ```
#[derive(Debug, Default, Clone, Resource, PartialEq, TypePath)]
pub struct ComponentRegistry {
    pub component_id_to_kind: HashMap<ComponentId, ComponentKind>,
    pub kind_to_component_id: HashMap<ComponentKind, ComponentId>,
    pub replication_map: HashMap<ComponentKind, ReplicationMetadata>,
    pub serialize_fns_map: HashMap<ComponentKind, ErasedSerializeFns>,
    pub(crate) delta_fns_map: HashMap<ComponentKind, ErasedDeltaFns>,
    pub kind_map: TypeMapper<ComponentKind>,
}

impl ComponentRegistry {
    pub fn net_id<C: 'static>(&self) -> ComponentNetId {
        self.kind_map
            .net_id(&ComponentKind::of::<C>())
            .copied()
            .unwrap_or_else(|| {
                panic!(
                    "Component {} is not registered",
                    core::any::type_name::<C>()
                )
            })
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
        // for component_kind in self.prediction_map.keys() {
        //     if !self.serialize_fns_map.contains_key(component_kind) {
        //         panic!(
        //             "A component has prediction metadata but wasn't registered for serialization"
        //         );
        //     }
        // }
        // for (component_kind, interpolation_data) in &self.interpolation_map {
        //     if interpolation_data.interpolation_mode == ComponentSyncMode::Full
        //         && interpolation_data.interpolation.is_none()
        //         && !interpolation_data.custom_interpolation
        //     {
        //         let name = self
        //             .serialize_fns_map
        //             .get(component_kind)
        //             .unwrap()
        //             .type_name;
        //         panic!("The Component {name:?} was registered for interpolation with ComponentSyncMode::FULL but no interpolation function was provided!");
        //     }
        // }
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
        self.component_id_to_kind
            .insert(component_id, component_kind);
        self.kind_to_component_id
            .insert(component_kind, component_id);
        self.serialize_fns_map.insert(
            component_kind,
            ErasedSerializeFns::new::<SendEntityMap, ReceiveEntityMap, C, C>(
                ContextSerializeFns::new(serialize_fns.serialize),
                ContextDeserializeFns::new(serialize_fns.deserialize),
            ),
        );
    }
}

fn mapped_context_serialize<M: MapEntities + Clone>(
    mapper: &mut SendEntityMap,
    message: &M,
    writer: &mut Writer,
    serialize_fn: SerializeFn<M>,
) -> core::result::Result<(), SerializationError> {
    // TODO: this is actually UB, we can never have 2 aliasing &mut
    // SAFETY: we know that the entity mapper is not actually being mutated
    let mut message = message.clone();
    message.map_entities(mapper);
    serialize_fn(&message, writer)
}

fn mapped_context_deserialize<M: MapEntities>(
    mapper: &mut ReceiveEntityMap,
    reader: &mut Reader,
    deserialize_fn: DeserializeFn<M>,
) -> core::result::Result<M, SerializationError> {
    let mut message = deserialize_fn(reader)?;
    message.map_entities(mapper);
    Ok(message)
}

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
                core::any::type_name::<C>()
            )
        });
        erased_fns.add_map_entities::<C>();
        let context_serialize: ContextSerializeFn<SendEntityMap, C, C> =
            mapped_context_serialize::<C>;
        let context_deserialize: ContextDeserializeFn<ReceiveEntityMap, C, C> =
            mapped_context_deserialize::<C>;
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
    pub(crate) fn raw_deserialize<C: 'static>(
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
        unsafe { erased_fns.deserialize::<_, C, C>(reader, entity_map) }.map_err(Into::into)
    }

    pub(crate) fn deserialize<C: Component>(
        &self,
        reader: &mut Reader,
        entity_map: &mut ReceiveEntityMap,
    ) -> Result<C, ComponentError> {
        let net_id = NetId::from_bytes(reader)?;
        self.raw_deserialize(reader, entity_map)
    }

    pub fn map_entities<C: 'static>(
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

/// Add a component to the list of components that can be sent
pub trait AppComponentExt {
    /// Registers the component in the Registry
    /// This component can now be sent over the network.
    fn register_component<
        C: Component<Mutability: GetWriteFns<C>> + Serialize + DeserializeOwned + PartialEq,
    >(
        &mut self,
    ) -> ComponentRegistration<'_, C>;

    /// Registers the component in the Registry: this component can now be sent over the network.
    ///
    /// You need to provide your own [`SerializeFns`]
    fn register_component_custom_serde<C: Component<Mutability: GetWriteFns<C>> + PartialEq>(
        &mut self,
        serialize_fns: SerializeFns<C>,
    ) -> ComponentRegistration<'_, C>;
}

impl AppComponentExt for App {
    fn register_component<
        C: Component<Mutability: GetWriteFns<C>> + PartialEq + Serialize + DeserializeOwned,
    >(
        &mut self,
    ) -> ComponentRegistration<'_, C> {
        self.register_component_custom_serde(SerializeFns::<C>::default())
    }

    fn register_component_custom_serde<C: Component<Mutability: GetWriteFns<C>> + PartialEq>(
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
                debug!("register component {}", core::any::type_name::<C>());
            });
        ComponentRegistration {
            app: self,
            _phantom: core::marker::PhantomData,
        }
        .with_replication_config(ComponentReplicationConfig::default())
    }
}

pub struct ComponentRegistration<'a, C> {
    pub app: &'a mut App,
    _phantom: core::marker::PhantomData<C>,
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

    pub fn with_replication_config(self, config: ComponentReplicationConfig) -> Self
    where
        C: Component<Mutability: GetWriteFns<C>> + PartialEq,
    {
        let overrides_component_id = self
            .app
            .world_mut()
            .register_component::<ComponentReplicationOverrides<C>>();
        let mut registry = self.app.world_mut().resource_mut::<ComponentRegistry>();
        registry.replication_map.insert(
            ComponentKind::of::<C>(),
            ReplicationMetadata::default_fns::<C>(
                config,
                overrides_component_id,
            ),
        );
        self
    }

    /// Enable delta compression when serializing this component
    pub fn add_delta_compression(self) -> Self
    where
        C: Component<Mutability = Mutable> + PartialEq + Diffable,
        C::Delta: Serialize + DeserializeOwned,
    {
        self.app
            .world_mut()
            .resource_scope(|world, mut registry: Mut<ComponentRegistry>| {
                registry.set_delta_compression::<C>(world);
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
