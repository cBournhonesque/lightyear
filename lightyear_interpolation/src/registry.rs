use crate::InterpolationMode;
use bevy::math::Curve;
use bevy::platform::collections::HashMap;
use bevy::prelude::{Component, Ease, EaseFunction, EasingCurve, Resource};
use core::ops::{Add, Mul};
use lightyear_replication::registry::registry::LerpFn;
use lightyear_replication::registry::ComponentKind;

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
        let interpolation_fn: LerpFn<C> = unsafe { core::mem::transmute(interpolation_metadata.interpolation.unwrap()) };
        interpolation_fn(start, end, t)
    }
}
