use crate::registry::ComponentRegistry;
use bevy_app::App;
use bevy_ecs::change_detection::Mut;
use bevy_ecs::component::Component;
use bevy_ecs::resource::Resource;
use bevy_replicon::prelude::{AppRuleExt, ReplicationMode, RuleFns};
use bevy_replicon::shared::replication::diff::Diffable as RepliconDiffable;
use bevy_replicon::shared::replication::registry::receive_fns::MutWrite;
use bevy_replicon::shared::replication::registry::rule_fns::{DeserializeFn, SerializeFn};
use bevy_replicon::shared::replication::rules::filter::FilterRules;
use serde::{Serialize, de::DeserializeOwned};

/// Add a component to the list of components that can be sent
pub trait AppComponentExt {
    /// Registers the component in Lightyear's component metadata registry only.
    ///
    /// This does not add any Replicon replication rule. Use this when you want
    /// to call Replicon's replication APIs directly or through this builder:
    ///
    /// ```rust,ignore
    /// app.replicate::<MyComponent>();
    /// app.component::<MyComponent>().predict();
    ///
    /// app.component::<MyComponent>()
    ///     .replicate()
    ///     .predict();
    /// ```
    ///
    /// This also works with Replicon's custom registration APIs such as
    /// `replicate_with_priority`, `replicate_with_priority_filtered`,
    /// `replicate_as`, and `replicate_with`.
    fn component<C: Component>(&mut self) -> ComponentRegistration<'_, C>;

    /// Registers a Bevy resource in Lightyear's component metadata registry.
    ///
    /// In Bevy 0.19 resources are components stored on resource entities, so
    /// this returns the same builder as [`AppComponentExt::component`] with a
    /// resource-specific bound.
    fn resource<R: Resource>(&mut self) -> ComponentRegistration<'_, R>;

    /// Registers the component in the Registry
    /// This component can now be sent over the network.
    #[deprecated(note = "use `app.component::<C>().replicate()` instead")]
    fn register_component<C: Component<Mutability: MutWrite<C>> + Serialize + DeserializeOwned>(
        &mut self,
    ) -> ComponentRegistration<'_, C>;

    /// Registers the component using Replicon's diff-based replication.
    #[deprecated(note = "use `app.component::<C>().replicate_diff()` instead")]
    fn register_component_diff<C: RepliconDiffable>(&mut self) -> ComponentRegistration<'_, C>;

    /// Registers the component in the Registry with `ReplicationMode::Once`.
    ///
    /// This component can now be sent over the network, but only insertions and
    /// removals are replicated. Component mutations are not sent.
    #[deprecated(note = "use `app.component::<C>().replicate_once()` instead")]
    fn register_component_once<
        C: Component<Mutability: MutWrite<C>> + Serialize + DeserializeOwned,
    >(
        &mut self,
    ) -> ComponentRegistration<'_, C>;

    /// Registers the component in the Registry: this component can now be sent over the network.
    ///
    /// You need to provide your own serialization functions.
    #[deprecated(note = "use `app.component::<C>().replicate_with(...)` instead")]
    fn register_component_with<C: Component<Mutability: MutWrite<C>>>(
        &mut self,
        serialize_fn: SerializeFn<C>,
        deserialize_fn: DeserializeFn<C>,
    ) -> ComponentRegistration<'_, C>;

    /// Registers the component in the Registry with custom serialization and
    /// `ReplicationMode::Once`.
    #[deprecated(note = "use `app.component::<C>().replicate_once_with(...)` instead")]
    fn register_component_once_with<C: Component<Mutability: MutWrite<C>>>(
        &mut self,
        serialize_fn: SerializeFn<C>,
        deserialize_fn: DeserializeFn<C>,
    ) -> ComponentRegistration<'_, C>;

    /// Returns a ComponentRegistration for a component that is not networked.
    ///
    /// This can be useful for components that are not networked but that you still need
    /// to sync to predicted or interpolated entities; or for which you need to enable
    /// rollback.
    #[deprecated(note = "use `app.local_rollback::<C>()` for non-networked rollback")]
    fn non_networked_component<C: Component<Mutability: MutWrite<C>>>(
        &mut self,
    ) -> ComponentRegistration<'_, C>;
}

impl AppComponentExt for App {
    fn component<C: Component>(&mut self) -> ComponentRegistration<'_, C> {
        register_component_metadata::<C>(self);
        ComponentRegistration::new(self)
    }

    fn resource<R: Resource>(&mut self) -> ComponentRegistration<'_, R> {
        self.component::<R>()
    }

    fn register_component<C: Component<Mutability: MutWrite<C>> + Serialize + DeserializeOwned>(
        &mut self,
    ) -> ComponentRegistration<'_, C> {
        self.component::<C>().replicate()
    }

    fn register_component_diff<C: RepliconDiffable>(&mut self) -> ComponentRegistration<'_, C> {
        self.component::<C>().replicate_diff()
    }

    fn register_component_once<
        C: Component<Mutability: MutWrite<C>> + Serialize + DeserializeOwned,
    >(
        &mut self,
    ) -> ComponentRegistration<'_, C> {
        self.component::<C>().replicate_once()
    }

    fn register_component_with<C: Component<Mutability: MutWrite<C>>>(
        &mut self,
        serialize_fn: SerializeFn<C>,
        deserialize_fn: DeserializeFn<C>,
    ) -> ComponentRegistration<'_, C> {
        self.component::<C>()
            .replicate_with(RuleFns::new(serialize_fn, deserialize_fn))
    }

    fn register_component_once_with<C: Component<Mutability: MutWrite<C>>>(
        &mut self,
        serialize_fn: SerializeFn<C>,
        deserialize_fn: DeserializeFn<C>,
    ) -> ComponentRegistration<'_, C> {
        self.component::<C>()
            .replicate_once_with(RuleFns::new(serialize_fn, deserialize_fn))
    }

    fn non_networked_component<C: Component<Mutability: MutWrite<C>>>(
        &mut self,
    ) -> ComponentRegistration<'_, C> {
        ComponentRegistration::new(self)
    }
}

fn register_component_metadata<C: Component>(app: &mut App) {
    if app
        .world_mut()
        .get_resource_mut::<ComponentRegistry>()
        .is_none()
    {
        app.world_mut().init_resource::<ComponentRegistry>();
    }
    app.world_mut()
        .resource_scope(|world, mut registry: Mut<ComponentRegistry>| {
            if !registry.is_registered::<C>() {
                registry.register_component::<C>(world);
            }
        });
}

pub struct ComponentRegistration<'a, C> {
    pub app: &'a mut App,
    _phantom: core::marker::PhantomData<C>,
}

/// A builder state that can be converted to and from the base component
/// registration builder.
///
/// Extension traits use this to compose registration domains without requiring
/// callers to manually unwrap typed builder states.
pub trait ComponentRegistrator<'a, C>: Sized {
    fn into_component_registration(self) -> ComponentRegistration<'a, C>;
    fn from_component_registration(registration: ComponentRegistration<'a, C>) -> Self;
}

impl<'a, C> ComponentRegistrator<'a, C> for ComponentRegistration<'a, C> {
    fn into_component_registration(self) -> ComponentRegistration<'a, C> {
        self
    }

    fn from_component_registration(registration: ComponentRegistration<'a, C>) -> Self {
        registration
    }
}

impl<C> ComponentRegistration<'_, C> {
    pub fn new(app: &mut App) -> ComponentRegistration<'_, C> {
        ComponentRegistration {
            app,
            _phantom: core::marker::PhantomData,
        }
    }

    /// Register this component with Replicon's default `OnChange` replication.
    pub fn replicate(self) -> Self
    where
        C: Component<Mutability: MutWrite<C>> + Serialize + DeserializeOwned,
    {
        self.app.replicate::<C>();
        self
    }

    /// Register this component with Replicon's `Once` replication mode.
    pub fn replicate_once(self) -> Self
    where
        C: Component<Mutability: MutWrite<C>> + Serialize + DeserializeOwned,
    {
        self.app.replicate_once::<C>();
        self
    }

    /// Register this component using Replicon's diff-based replication.
    ///
    /// Mutations must be recorded with
    /// [`EntityDiffExt::apply_diff`](bevy_replicon::shared::replication::diff::EntityDiffExt::apply_diff)
    /// so Replicon can create diff messages. This registers Replicon's
    /// `replicate_diff` rule, but does not also register the normal
    /// full-component replication rule.
    pub fn replicate_diff(self) -> Self
    where
        C: RepliconDiffable,
    {
        self.app.replicate_diff::<C>();
        self
    }

    /// Register this component with Replicon's default replication and an
    /// archetype filter.
    pub fn replicate_filtered<F: FilterRules>(self) -> Self
    where
        C: Component<Mutability: MutWrite<C>> + Serialize + DeserializeOwned,
    {
        self.app.replicate_filtered::<C, F>();
        self
    }

    /// Register this component with Replicon's `Once` replication mode and an
    /// archetype filter.
    pub fn replicate_once_filtered<F: FilterRules>(self) -> Self
    where
        C: Component<Mutability: MutWrite<C>> + Serialize + DeserializeOwned,
    {
        self.app.replicate_once_filtered::<C, F>();
        self
    }

    /// Register this component with Replicon's `replicate_as` conversion API.
    pub fn replicate_as<T>(self) -> Self
    where
        C: Component<Mutability: MutWrite<C>> + Clone + Into<T> + From<T>,
        T: Serialize + DeserializeOwned,
    {
        self.app.replicate_as::<C, T>();
        self
    }

    /// Register this component with Replicon's `replicate_once_as` conversion API.
    pub fn replicate_once_as<T>(self) -> Self
    where
        C: Component<Mutability: MutWrite<C>> + Clone + Into<T> + From<T>,
        T: Serialize + DeserializeOwned,
    {
        self.app.replicate_once_as::<C, T>();
        self
    }

    /// Register this component with Replicon's filtered conversion API.
    pub fn replicate_filtered_as<T, F: FilterRules>(self) -> Self
    where
        C: Component<Mutability: MutWrite<C>> + Clone + Into<T> + From<T>,
        T: Serialize + DeserializeOwned,
    {
        self.app.replicate_filtered_as::<C, T, F>();
        self
    }

    /// Register this component with Replicon's filtered `Once` conversion API.
    pub fn replicate_once_filtered_as<T, F: FilterRules>(self) -> Self
    where
        C: Component<Mutability: MutWrite<C>> + Clone + Into<T> + From<T>,
        T: Serialize + DeserializeOwned,
    {
        self.app.replicate_once_filtered_as::<C, T, F>();
        self
    }

    /// Register this component with custom Replicon rule functions.
    pub fn replicate_with(self, rule_fns: RuleFns<C>) -> Self
    where
        C: Component<Mutability: MutWrite<C>>,
    {
        self.app.replicate_with(rule_fns);
        self
    }

    /// Register this component with custom Replicon rule functions and
    /// `ReplicationMode::Once`.
    pub fn replicate_once_with(self, rule_fns: RuleFns<C>) -> Self
    where
        C: Component<Mutability: MutWrite<C>>,
    {
        self.app.replicate_with((rule_fns, ReplicationMode::Once));
        self
    }

    /// Register this component with custom Replicon rule functions and an
    /// archetype filter.
    pub fn replicate_with_filtered<F: FilterRules>(self, rule_fns: RuleFns<C>) -> Self
    where
        C: Component<Mutability: MutWrite<C>>,
    {
        self.app.replicate_with_filtered::<_, F>(rule_fns);
        self
    }

    /// Register this component with custom Replicon rule functions,
    /// `ReplicationMode::Once`, and an archetype filter.
    pub fn replicate_once_with_filtered<F: FilterRules>(self, rule_fns: RuleFns<C>) -> Self
    where
        C: Component<Mutability: MutWrite<C>>,
    {
        self.app
            .replicate_with_filtered::<_, F>((rule_fns, ReplicationMode::Once));
        self
    }

    /// Register this component with Replicon's default rule functions and a
    /// custom priority.
    pub fn replicate_with_priority(self, priority: usize) -> Self
    where
        C: Component<Mutability: MutWrite<C>> + Serialize + DeserializeOwned,
    {
        self.app
            .replicate_with_priority(priority, RuleFns::<C>::default());
        self
    }

    /// Register this component with Replicon's default rule functions, a custom
    /// priority, and an archetype filter.
    pub fn replicate_with_priority_filtered<F: FilterRules>(self, priority: usize) -> Self
    where
        C: Component<Mutability: MutWrite<C>> + Serialize + DeserializeOwned,
    {
        self.app
            .replicate_with_priority_filtered::<_, F>(priority, RuleFns::<C>::default());
        self
    }

    /// Register this component with custom Replicon rule functions and a custom
    /// priority.
    pub fn replicate_custom_with_priority(self, priority: usize, rule_fns: RuleFns<C>) -> Self
    where
        C: Component<Mutability: MutWrite<C>>,
    {
        self.app.replicate_with_priority(priority, rule_fns);
        self
    }

    /// Register this component with custom Replicon rule functions, a custom
    /// priority, and an archetype filter.
    pub fn replicate_custom_with_priority_filtered<F: FilterRules>(
        self,
        priority: usize,
        rule_fns: RuleFns<C>,
    ) -> Self
    where
        C: Component<Mutability: MutWrite<C>>,
    {
        self.app
            .replicate_with_priority_filtered::<_, F>(priority, rule_fns);
        self
    }

    /// Deprecated compatibility shim for the removed delta-compression registration path.
    ///
    /// Replicon-backed component replication does not currently use Lightyear's
    /// old delta-compression metadata, so this preserves the old builder chain
    /// without changing the Replicon rule registered by earlier builder calls.
    #[cfg(feature = "delta")]
    #[deprecated(
        note = "Lightyear delta compression is not implemented on the Replicon backend; use `replicate_diff()` for Replicon diff replication"
    )]
    pub fn add_delta_compression<Delta>(self) -> Self
    where
        C: crate::delta::Diffable<Delta>,
        Delta: Serialize + DeserializeOwned,
    {
        self
    }
}

#[derive(Debug, Default, Clone)]
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

#[cfg(test)]
mod tests {
    use super::*;
    use bevy_ecs::prelude::With;
    use bevy_replicon::prelude::{AuthMethod, RepliconSharedPlugin};
    use bevy_replicon::shared::replication::rules::ReplicationRules;
    use bevy_state::app::StatesPlugin;

    #[derive(Component, Serialize, serde::Deserialize)]
    struct DirectRepliconComponent;

    #[derive(Component, Serialize, serde::Deserialize)]
    struct PriorityComponent;

    #[derive(Component)]
    struct PriorityFilter;

    #[derive(Component)]
    struct MetadataOnlyComponent;

    #[test]
    fn component_registers_lightyear_metadata_without_replicon_rule() {
        let mut app = App::new();

        app.component::<MetadataOnlyComponent>();

        let registry = app.world().resource::<ComponentRegistry>();
        assert!(registry.is_registered::<MetadataOnlyComponent>());
    }

    #[test]
    fn component_can_be_used_after_direct_replicon_registration() {
        let mut app = App::new();
        app.add_plugins((
            StatesPlugin,
            RepliconSharedPlugin {
                auth_method: AuthMethod::None,
            },
        ));

        app.replicate::<DirectRepliconComponent>();
        app.component::<DirectRepliconComponent>();

        let registry = app.world().resource::<ComponentRegistry>();
        assert!(registry.is_registered::<DirectRepliconComponent>());
    }

    #[test]
    fn component_can_be_used_after_custom_direct_replicon_registration() {
        let mut app = App::new();
        app.add_plugins((
            StatesPlugin,
            RepliconSharedPlugin {
                auth_method: AuthMethod::None,
            },
        ));

        app.component::<PriorityComponent>()
            .replicate_with_priority_filtered::<With<PriorityFilter>>(7);

        let registry = app.world().resource::<ComponentRegistry>();
        assert!(registry.is_registered::<PriorityComponent>());

        let rules = app.world().resource::<ReplicationRules>();
        assert!(rules.iter().any(|rule| rule.priority == 7));
    }
}
