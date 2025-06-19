use crate::{
    InterpolationMode, SyncComponent, add_interpolation_systems, add_prepare_interpolation_systems,
};
use bevy::math::Curve;
use bevy::platform::collections::HashMap;
use bevy::prelude::{Component, Ease, EaseFunction, EasingCurve, Resource};
use lightyear_replication::prelude::ComponentRegistration;
use lightyear_replication::registry::ComponentKind;
use lightyear_replication::registry::registry::LerpFn;

fn lerp<C: Ease + Clone>(start: C, other: C, t: f32) -> C {
    let curve = EasingCurve::new(start, other, EaseFunction::Linear);
    curve.sample_unchecked(t)
}

#[derive(Debug, Clone, PartialEq)]
pub struct InterpolationMetadata {
    pub interpolation_mode: InterpolationMode,
    pub interpolation: Option<unsafe fn()>,
    pub custom_interpolation: bool,
}

#[derive(Resource, Debug, Default)]
pub struct InterpolationRegistry {
    pub(crate) interpolation_map: HashMap<ComponentKind, InterpolationMetadata>,
}

impl InterpolationRegistry {
    pub fn set_interpolation_mode<C: Component>(&mut self, mode: InterpolationMode) {
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

    pub fn set_linear_interpolation<C: Component + Clone + Ease>(&mut self) {
        self.set_interpolation(lerp::<C>);
    }

    pub fn set_interpolation<C: Component>(&mut self, interpolation_fn: LerpFn<C>) {
        let kind = ComponentKind::of::<C>();
        self.interpolation_map
            .entry(kind)
            .or_insert_with(|| InterpolationMetadata {
                interpolation_mode: InterpolationMode::Full,
                interpolation: None,
                custom_interpolation: false,
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
}

pub trait InterpolationRegistrationExt<C> {
    /// Enable interpolation systems for this component.
    /// You can specify the interpolation [`InterpolationMode`]
    fn add_interpolation(self, interpolation_mode: InterpolationMode) -> Self
    where
        C: SyncComponent;
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
    fn add_interpolation(self, interpolation_mode: InterpolationMode) -> Self
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
