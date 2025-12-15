use bevy_app::App;
use bevy_ecs::change_detection::Mut;
use bevy_ecs::component::{Component, Mutable};
use bevy_ecs::entity::MapEntities;
use bevy_replicon::prelude::{AppRuleExt, RuleFns};
use bevy_replicon::shared::replication::registry::command_fns::MutWrite;
use bevy_replicon::shared::replication::registry::rule_fns::{DeserializeFn, SerializeFn};
use lightyear_serde::registry::SerializeFns;
use serde::{Serialize, DeserializeOwned};

/// Add a component to the list of components that can be sent
pub trait AppComponentExt {
    /// Registers the component in the Registry
    /// This component can now be sent over the network.
    fn register_component<C: Component<Mutability: MutWrite<C>> + Serialize + DeserializeOwned>(
        &mut self,
    ) -> ComponentRegistration<'_, C>;

    /// Registers the component in the Registry: this component can now be sent over the network.
    ///
    /// You need to provide your own [`SerializeFns`]
    fn register_component_with<C: Component<Mutability: MutWrite<C>>>(
        &mut self,
        serialize_fn: SerializeFn<C>,
        deserialize_fn: DeserializeFn<C>,
    ) -> ComponentRegistration<'_, C>;

    /// Returns a ComponentRegistration for a component that is not networked.
    ///
    /// This can be useful for components that are not networked but that you still need
    /// to sync to predicted or interpolated entities; or for which you need to enable
    /// rollback.
    fn non_networked_component<C: Component<Mutability: MutWrite<C>>>(
        &mut self,
    ) -> ComponentRegistration<'_, C>;
}

impl AppComponentExt for App {
    fn register_component<
        C: Component<Mutability: MutWrite<C>> + Serialize + DeserializeOwned,
    >(
        &mut self,
    ) -> ComponentRegistration<'_, C> {
        self.replicate::<C>();
        ComponentRegistration::new(self)
    }

    fn register_component_with<C: Component<Mutability: MutWrite<C>>>(
        &mut self,
        serialize_fn: SerializeFn<C>,
        deserialize_fn: DeserializeFn<C>,
    ) -> ComponentRegistration<'_, C> {

        self.replicate_with(RuleFns::new(
            serialize_fn,
            deserialize_fn
        ));
        ComponentRegistration::new(self)
    }

    fn non_networked_component<C: Component<Mutability: GetWriteFns<C>>>(
        &mut self,
    ) -> ComponentRegistration<'_, C> {
        ComponentRegistration::new(self)
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
            core::panic!(
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