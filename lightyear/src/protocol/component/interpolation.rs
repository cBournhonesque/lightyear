use crate::client::components::ComponentSyncMode;
use crate::prelude::ComponentRegistry;
use crate::protocol::component::registry::LerpFn;
use crate::protocol::component::ComponentKind;
use bevy::prelude::Component;
use core::ops::{Add, Mul};

#[derive(Debug, Clone, PartialEq)]
pub struct InterpolationMetadata {
    pub interpolation_mode: ComponentSyncMode,
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

impl ComponentRegistry {
    pub fn set_interpolation_mode<C: Component>(&mut self, mode: ComponentSyncMode) {
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

    pub fn set_linear_interpolation<C: Component + Linear>(&mut self) {
        self.set_interpolation(<C as Linear>::lerp);
    }

    pub fn set_interpolation<C: Component>(&mut self, interpolation_fn: LerpFn<C>) {
        let kind = ComponentKind::of::<C>();
        self.interpolation_map
            .entry(kind)
            .or_insert_with(|| InterpolationMetadata {
                interpolation_mode: ComponentSyncMode::Full,
                interpolation: None,
                custom_interpolation: false,
            })
            .interpolation = Some(unsafe {
            core::mem::transmute::<for<'a, 'b> fn(&'a C, &'b C, f32) -> C, unsafe fn()>(
                interpolation_fn,
            )
        });
    }

    pub fn interpolation_mode<C: Component>(&self) -> ComponentSyncMode {
        let kind = ComponentKind::of::<C>();
        self.interpolation_map
            .get(&kind)
            .map_or(ComponentSyncMode::None, |metadata| {
                metadata.interpolation_mode
            })
    }
    pub fn interpolate<C: Component>(&self, start: &C, end: &C, t: f32) -> C {
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
