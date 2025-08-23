use crate::plugin::{
    add_immutable_prepare_interpolation_systems, add_interpolation_systems,
    add_prepare_interpolation_systems,
};
use crate::{InterpolationMode, SyncComponent};
use bevy_ecs::{component::Component, resource::Resource};
use bevy_ecs::component::ComponentId;
use bevy_ecs::entity::Entity;
use bevy_ecs::prelude::World;
use bevy_math::{
    Curve,
    curve::{Ease, EaseFunction, EasingCurve},
};
use bevy_platform::collections::HashMap;
use lightyear_replication::prelude::{ComponentRegistration, ComponentRegistry};
use lightyear_replication::registry::buffered::BufferedChanges;
use lightyear_replication::registry::{ComponentError, ComponentKind};
use lightyear_replication::registry::registry::LerpFn;
use crate::manager::InterpolationManager;

fn lerp<C: Ease + Clone>(start: C, other: C, t: f32) -> C {
    let curve = EasingCurve::new(start, other, EaseFunction::Linear);
    curve.sample_unchecked(t)
}

/// Function that will sync a component value from the confirmed entity to the interpolated entity
type SyncFn = fn(
    &InterpolationRegistry,
    &ComponentRegistry,
    confirmed: Entity,
    predicted: Entity,
    manager: Entity,
    &World,
    &mut BufferedChanges,
);

#[derive(Debug, Clone)]
pub struct InterpolationMetadata {
    pub interpolation_mode: InterpolationMode,
    pub interpolation: Option<unsafe fn()>,
    pub custom_interpolation: bool,
    pub buffer_sync: SyncFn,
}

#[derive(Resource, Debug, Default)]
pub struct InterpolationRegistry {
    pub(crate) interpolation_map: HashMap<ComponentKind, InterpolationMetadata>,
}

impl InterpolationRegistry {
    pub fn set_interpolation_mode<C: Component + Clone>(&mut self, mode: InterpolationMode) {
        let kind = ComponentKind::of::<C>();
        self.interpolation_map
            .entry(kind)
            .or_insert_with(|| InterpolationMetadata {
                interpolation_mode: mode,
                interpolation: None,
                custom_interpolation: false,
                buffer_sync: Self::buffer_sync::<C>,
            })
            .interpolation_mode = mode;
    }

    pub fn set_linear_interpolation<C: Component + Clone + Ease>(&mut self) {
        self.set_interpolation(lerp::<C>);
    }

    pub fn set_interpolation<C: Component + Clone>(&mut self, interpolation_fn: LerpFn<C>) {
        let kind = ComponentKind::of::<C>();
        self.interpolation_map
            .entry(kind)
            .or_insert_with(|| InterpolationMetadata {
                interpolation_mode: InterpolationMode::Full,
                interpolation: None,
                custom_interpolation: false,
                buffer_sync: Self::buffer_sync::<C>,
            })
            .interpolation = Some(unsafe { core::mem::transmute(interpolation_fn) });
    }

    pub fn interpolation_mode<C: Component>(&self) -> InterpolationMode {
        let kind = ComponentKind::of::<C>();
        self.interpolation_map
            .get(&kind)
            .map_or(InterpolationMode::None, |metadata| {
                metadata.interpolation_mode
            })
    }

    pub(crate) fn get_interpolation_mode(
        &self,
        id: ComponentId,
        component_registry: &ComponentRegistry,
    ) -> Result<InterpolationMode, ComponentError> {
        let kind = component_registry
            .component_id_to_kind
            .get(&id)
            .ok_or(ComponentError::NotRegistered)?;
        Ok(self
            .interpolation_map
            .get(kind)
            .map_or(InterpolationMode::None, |metadata| metadata.interpolation_mode))
    }

    pub fn interpolate<C: Component>(&self, start: C, end: C, t: f32) -> C {
        let kind = ComponentKind::of::<C>();
        let interpolation_metadata = self
            .interpolation_map
            .get(&kind)
            .expect("the component is not part of the protocol");
        let interpolation_fn: LerpFn<C> =
            unsafe { core::mem::transmute(interpolation_metadata.interpolation.unwrap()) };
        interpolation_fn(start, end, t)
    }
    
    // TODO: also sync removals!
    /// Clone the components from the confirmed entity to the interpolated entity
    /// All the cloned components are inserted at once.
    pub(crate) fn batch_sync(
        &self,
        component_registry: &ComponentRegistry,
        component_ids: &[ComponentId],
        confirmed: Entity,
        predicted: Entity,
        manager: Entity,
        world: &mut World,
        buffer: &mut BufferedChanges,
    ) {
        // clone each component to be synced into a temporary buffer
        component_ids.iter().for_each(|component_id| {
            let kind = component_registry
                .component_id_to_kind
                .get(component_id)
                .unwrap();
            let interpolated_metadata = self
                .interpolation_map
                .get(kind)
                .expect("the component is not part of the protocol");
            (interpolated_metadata.buffer_sync)(
                self,
                component_registry,
                confirmed,
                predicted,
                manager,
                world,
                buffer,
            );
        });
        // insert all the components in the predicted entity
        if let Ok(mut entity_world_mut) = world.get_entity_mut(predicted) {
            buffer.apply(&mut entity_world_mut);
        };
    }

    /// Sync a component value from the confirmed entity to the interpolated entity
    fn buffer_sync<C: Component + Clone>(
        &self,
        component_registry: &ComponentRegistry,
        confirmed: Entity,
        interpolated: Entity,
        manager: Entity,
        world: &World,
        buffer: &mut BufferedChanges,
    ) {
        let kind = ComponentKind::of::<C>();
        let interpolation_metadata = self
            .interpolation_map
            .get(&kind)
            .expect("the component is not part of the protocol");

        // NOTE: this is not needed because we have an observer that inserts the History as soon as C is inserted.
        // // for Full components, also insert a PredictionHistory component
        // // no need to add any value to it because otherwise it would contain a value with the wrong tick
        // // since we are running this outside of FixedUpdate
        // if prediction_metadata.prediction_mode == PredictionMode::Full {
        //     // if the predicted entity already had a PredictionHistory component (for example
        //     // if the entity was PreSpawned entity), we don't want to overwrite it.
        //     if world.get::<PredictionHistory<C>>(predicted).is_none() {
        //         unsafe {
        //             self.temp_write_buffer.buffer_insert_raw_ptrs(
        //                 PredictionHistory::<C>::default(),
        //                 world
        //                     .component_id::<PredictionHistory<C>>()
        //                     .expect("PredictionHistory not registered"),
        //             )
        //         };
        //     }
        // }
        
        // InterpolationMode::Full: we don't want to sync the component directly, but we want to insert the InterpolationHistory
        //  (we don't want to sync the component value directly because it would be too early; we want to only add the component
        //   when it interpolates between two updates)
        // InterpolationMode::Once, we only need to sync it once
        // InterpolationMode::Simple, every component update will be synced via a separate system
        if world.get::<C>(interpolated).is_some() {
            return;
        }
        if let Some(value) = world.get::<C>(confirmed) {
            let mut clone = value.clone();
            world
                .get::<InterpolationManager>(manager)
                .unwrap()
                .map_entities(&mut clone, component_registry)
                .unwrap();
            // SAFETY: the component_id matches the component of type C
            unsafe {
                buffer.insert::<C>(clone, world.component_id::<C>().unwrap());
            };
        }
    }
}

pub trait InterpolationRegistrationExt<C> {
    /// Enable interpolation systems for this component.
    /// You can specify the interpolation [`InterpolationMode`]
    fn add_interpolation(self, interpolation_mode: InterpolationMode) -> Self
    where
        C: SyncComponent;

    /// Enable interpolation systems for this immutable component.
    /// You can specify the interpolation [`InterpolationMode`]
    ///
    /// Note that [`InterpolationMode::Full`] is not supported for immutable components.
    fn add_immutable_interpolation(self, interpolation_mode: InterpolationMode) -> Self
    where
        C: Component + Clone;

    /// Register helper systems to perform interpolation for the component; but the user has to define the interpolation logic
    /// themselves (the interpolation_fn will not be used)
    fn add_custom_interpolation(self, interpolation_mode: InterpolationMode) -> Self
    where
        C: SyncComponent;
    /// Add a `Interpolation` behaviour to this component by using a linear interpolation function.
    fn add_linear_interpolation_fn(self) -> Self
    where
        C: SyncComponent + Ease;

    /// Add a `Interpolation` behaviour to this component.
    fn add_interpolation_fn(self, interpolation_fn: LerpFn<C>) -> Self
    where
        C: SyncComponent;
}

impl<C> InterpolationRegistrationExt<C> for ComponentRegistration<'_, C> {
    /// Enable interpolation systems for this component.
    /// You can specify the interpolation [`InterpolationMode`]
    ///
    /// Note that [`InterpolationMode::Full`] is not supported for immutable components.
    fn add_immutable_interpolation(self, interpolation_mode: InterpolationMode) -> Self
    where
        C: Component + Clone,
    {
        assert_ne!(
            interpolation_mode,
            InterpolationMode::Full,
            "Full interpolation mode is not supported for immutable components"
        );
        if !self
            .app
            .world()
            .contains_resource::<InterpolationRegistry>()
        {
            self.app
                .world_mut()
                .insert_resource(InterpolationRegistry::default());
        }
        let mut registry = self.app.world_mut().resource_mut::<InterpolationRegistry>();

        registry.set_interpolation_mode::<C>(interpolation_mode);
        add_immutable_prepare_interpolation_systems::<C>(self.app, interpolation_mode);
        self
    }

    /// Enable interpolation systems for this component.
    /// You can specify the interpolation [`InterpolationMode`]
    fn add_interpolation(self, interpolation_mode: InterpolationMode) -> Self
    where
        C: SyncComponent,
    {
        if !self
            .app
            .world()
            .contains_resource::<InterpolationRegistry>()
        {
            self.app
                .world_mut()
                .insert_resource(InterpolationRegistry::default());
        }
        let mut registry = self.app.world_mut().resource_mut::<InterpolationRegistry>();

        registry.set_interpolation_mode::<C>(interpolation_mode);
        add_prepare_interpolation_systems::<C>(self.app, interpolation_mode);
        if interpolation_mode == InterpolationMode::Full {
            add_interpolation_systems::<C>(self.app);
        }
        self
    }

    /// Register helper systems to perform interpolation for the component; but the user has to define the interpolation logic
    /// themselves (the interpolation_fn will not be used)
    fn add_custom_interpolation(self, interpolation_mode: InterpolationMode) -> Self
    where
        C: SyncComponent,
    {
        let Some(mut registry) = self
            .app
            .world_mut()
            .get_resource_mut::<InterpolationRegistry>()
        else {
            return self;
        };
        registry.set_interpolation_mode::<C>(interpolation_mode);
        add_prepare_interpolation_systems::<C>(self.app, interpolation_mode);
        self
    }

    /// Add a `Interpolation` behaviour to this component by using a linear interpolation function.
    fn add_linear_interpolation_fn(self) -> Self
    where
        C: SyncComponent + Ease,
    {
        let Some(mut registry) = self
            .app
            .world_mut()
            .get_resource_mut::<InterpolationRegistry>()
        else {
            return self;
        };
        registry.set_linear_interpolation::<C>();
        self
    }

    /// Add a `Interpolation` behaviour to this component.
    fn add_interpolation_fn(self, interpolation_fn: LerpFn<C>) -> Self
    where
        C: SyncComponent,
    {
        let Some(mut registry) = self
            .app
            .world_mut()
            .get_resource_mut::<InterpolationRegistry>()
        else {
            return self;
        };
        registry.set_interpolation::<C>(interpolation_fn);
        self
    }
}
