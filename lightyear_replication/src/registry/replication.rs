use bevy_app::App;
use bevy_ecs::change_detection::Mut;
use bevy_ecs::component::{Component};
use bevy_replicon::prelude::{AppRuleExt, RuleFns};
use bevy_replicon::shared::replication::registry::command_fns::MutWrite;
use bevy_replicon::shared::replication::registry::rule_fns::{DeserializeFn, SerializeFn};
use serde::{Serialize, de::DeserializeOwned};
use crate::registry::ComponentRegistry;

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
                    registry.register_component::<C>(world);
                }
            });

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

    fn non_networked_component<C: Component<Mutability: MutWrite<C>>>(
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
}

#[derive(Debug, Clone)]
pub struct ReplicationMetadata {
    pub(crate) predicted: bool,
    pub(crate) interpolated: bool,
}

impl ReplicationMetadata {
    // TODO: Could we override this for a certain component? i.e. on an entity, the user can say
    //  "this component is not predicted"
    /// Mark the component as being predicted.
    pub fn set_predicted(&mut self, predicted: bool) {
        self.predicted = predicted;
    }

    /// Mark the component as being interpolated.
    pub fn set_interpolated(&mut self, interpolated: bool) {
        self.interpolated = interpolated;
    }
}