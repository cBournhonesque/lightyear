use crate::channel::builder::ChannelDirection;
use crate::client::components::{ComponentSyncMode, SyncComponent};
use crate::client::config::ClientConfig;
use crate::client::interpolation::plugin::{
    add_interpolation_systems, add_prepare_interpolation_systems,
};
use crate::client::prediction::plugin::{
    add_non_networked_rollback_systems, add_prediction_systems, add_resource_rollback_systems,
};
use crate::client::prediction::predicted_history::PredictionHistory;
use crate::packet::message::Message;
use crate::prelude::Linear;
use crate::protocol::component::delta::ErasedDeltaFns;
use crate::protocol::component::interpolation::InterpolationMetadata;
use crate::protocol::component::prediction::{PredictionMetadata, ShouldRollbackFn};
use crate::protocol::component::replication::{
    register_component_send, ReplicationMetadata, TempWriteBuffer,
};
use crate::protocol::component::{ComponentError, ComponentKind, ComponentNetId};
use crate::protocol::registry::{NetId, TypeMapper};
use crate::protocol::serialize::{ErasedSerializeFns, SerializeFns};
use crate::serialize::reader::Reader;
use crate::serialize::writer::Writer;
use crate::serialize::{SerializationError, ToBytes};
use crate::shared::replication::delta::Diffable;
use crate::shared::replication::entity_map::{EntityMap, ReceiveEntityMap, SendEntityMap};
use bevy::platform::collections::HashMap;
use bevy::app::App;
use bevy::ecs::change_detection::Mut;
use bevy::ecs::component::{Component, ComponentId, Mutable};
use bevy::ecs::entity::MapEntities;
use bevy::prelude::{Resource, TypePath, World};
use bevy::ptr::Ptr;
use serde::de::DeserializeOwned;
use serde::ser::Serialize;
use tracing::debug;

/// Function used to interpolate from one component state (`start`) to another (`other`)
/// t goes from 0.0 (`start`) to 1.0 (`other`)
pub type LerpFn<C> = fn(start: &C, other: &C, t: f32) -> C;

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
    pub(crate) temp_write_buffer: TempWriteBuffer,
    pub(crate) component_id_to_kind: HashMap<ComponentId, ComponentKind>,
    pub(crate) kind_to_component_id: HashMap<ComponentKind, ComponentId>,
    pub replication_map: HashMap<ComponentKind, ReplicationMetadata>,
    pub(crate) interpolation_map: HashMap<ComponentKind, InterpolationMetadata>,
    pub prediction_map: HashMap<ComponentKind, PredictionMetadata>,
    pub(crate) serialize_fns_map: HashMap<ComponentKind, ErasedSerializeFns>,
    pub(crate) delta_fns_map: HashMap<ComponentKind, ErasedDeltaFns>,
    pub kind_map: TypeMapper<ComponentKind>,
}

impl ComponentRegistry {
    pub fn net_id<C: 'static>(&self) -> ComponentNetId {
        self.kind_map
            .net_id(&ComponentKind::of::<C>())
            .copied()
            .unwrap_or_else(|| panic!("Component {} is not registered", core::any::type_name::<C>()))
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

    pub fn register_component<C: Component + Message + Serialize + DeserializeOwned>(
        &mut self,
        world: &mut World,
    ) {
        self.register_component_custom_serde(world, SerializeFns::<C>::default());
    }

    pub fn register_component_custom_serde<C: Component + Message>(
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
        component: &C,
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

/// Add a component to the list of components that can be sent
pub trait AppComponentExt {
    /// Registers the component in the Registry
    /// This component can now be sent over the network.
    fn register_component<
        C: Component<Mutability = Mutable> + Message + Serialize + DeserializeOwned + PartialEq,
    >(
        &mut self,
        direction: ChannelDirection,
    ) -> ComponentRegistration<'_, C>;

    /// Registers the component in the Registry: this component can now be sent over the network.
    ///
    /// You need to provide your own [`SerializeFns`]
    fn register_component_custom_serde<C: Component<Mutability = Mutable> + Message + PartialEq>(
        &mut self,
        direction: ChannelDirection,
        serialize_fns: SerializeFns<C>,
    ) -> ComponentRegistration<'_, C>;

    /// Enable rollbacks for a component even if the component is not networked
    fn add_rollback<C: Component<Mutability = Mutable> + PartialEq + Clone>(&mut self);

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
    fn add_delta_compression<C: Component<Mutability = Mutable> + PartialEq + Diffable>(&mut self)
    where
        C::Delta: Serialize + DeserializeOwned;
}

impl AppComponentExt for App {
    fn register_component<
        C: Component<Mutability = Mutable> + Message + PartialEq + Serialize + DeserializeOwned,
    >(
        &mut self,
        direction: ChannelDirection,
    ) -> ComponentRegistration<'_, C> {
        self.world_mut()
            .resource_scope(|world, mut registry: Mut<ComponentRegistry>| {
                if !registry.is_registered::<C>() {
                    debug!("register component {}", core::any::type_name::<C>());
                    registry.register_component::<C>(world);
                    registry.set_replication_fns::<C>(world, direction);
                }
            });
        register_component_send::<C>(self, direction);
        ComponentRegistration {
            app: self,
            _phantom: core::marker::PhantomData,
        }
    }

    fn register_component_custom_serde<C: Component<Mutability = Mutable> + Message + PartialEq>(
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
                debug!("register component {}", core::any::type_name::<C>());
            });
        register_component_send::<C>(self, direction);
        ComponentRegistration {
            app: self,
            _phantom: core::marker::PhantomData,
        }
    }

    // TODO: move this away from protocol? since it doesn't even use the registry at all
    //  maybe put this in the PredictionPlugin?
    fn add_rollback<C: Component<Mutability = Mutable> + PartialEq + Clone>(&mut self) {
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
        let history_id = (prediction_mode == ComponentSyncMode::Full).then(|| {
            self.world_mut()
                .register_component::<PredictionHistory<C>>()
        });
        let mut registry = self.world_mut().resource_mut::<ComponentRegistry>();
        registry.set_prediction_mode::<C>(history_id, prediction_mode);

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

    fn add_delta_compression<C: Component<Mutability = Mutable> + PartialEq + Diffable>(&mut self)
    where
        C::Delta: Serialize + DeserializeOwned,
    {
        self.world_mut()
            .resource_scope(|world, mut registry: Mut<ComponentRegistry>| {
                registry.set_delta_compression::<C>(world);
            })
    }
}

pub struct ComponentRegistration<'a, C> {
    app: &'a mut App,
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
        C: Component<Mutability = Mutable> + PartialEq + Diffable,
        C::Delta: Serialize + DeserializeOwned,
    {
        self.app.add_delta_compression::<C>();
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::serialize::writer::Writer;
    use crate::shared::replication::entity_map::SendEntityMap;
    use crate::tests::protocol::*;

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
}
