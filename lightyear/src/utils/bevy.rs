//! Implement lightyear traits for some common bevy types
use crate::_reexport::{InterpolatedComponent, LinearInterpolation};
use crate::client::components::{ComponentSyncMode, SyncComponent};
use crate::client::interpolation::InterpFn;
use bevy::prelude::{Entity, Transform};
use bevy::utils::EntityHashSet;
use std::ops::Mul;
use tracing::info;

use crate::prelude::{EntityMapper, MapEntities, Message, Named};

impl Named for Transform {
    fn name(&self) -> &'static str {
        "Transform"
    }
}

impl SyncComponent for Transform {
    fn mode() -> ComponentSyncMode {
        ComponentSyncMode::Full
    }
}

pub struct TransformLinearInterpolation;

impl InterpFn<Transform> for TransformLinearInterpolation {
    fn lerp(start: Transform, other: Transform, t: f32) -> Transform {
        let translation = start.translation * (1.0 - t) + other.translation * t;
        let rotation = start.rotation.lerp(other.rotation, t);
        let scale = start.scale * (1.0 - t) + other.scale * t;
        let res = Transform {
            translation,
            rotation,
            scale,
        };
        info!(
            "position lerp: start: {:?} end: {:?} t: {} res: {:?}",
            start, other, t, res
        );
        res
    }
}

impl InterpolatedComponent<Transform> for Transform {
    type Fn = TransformLinearInterpolation;
}

impl<'a> MapEntities<'a> for Transform {
    fn map_entities(&mut self, entity_mapper: Box<dyn EntityMapper + 'a>) {}

    fn entities(&self) -> EntityHashSet<Entity> {
        EntityHashSet::default()
    }
}

impl Message for Transform {}

cfg_if::cfg_if! {
    if #[cfg(feature = "render")] {
        use bevy::prelude::{Color,  Visibility};
        impl Named for Color {
            fn name(&self) -> &'static str {
                "Color"
            }
        }

        impl<'a> MapEntities<'a> for Color {
            fn map_entities(&mut self, entity_mapper: Box<dyn EntityMapper + 'a>) {}

            fn entities(&self) -> EntityHashSet<Entity> {
                EntityHashSet::default()
            }
        }

        impl Message for Color {}

        impl Named for Visibility {
            fn name(&self) -> &'static str {
                "Visibility"
            }
        }

        impl<'a> MapEntities<'a> for Visibility {
            fn map_entities(&mut self, entity_mapper: Box<dyn EntityMapper + 'a>) {}

            fn entities(&self) -> EntityHashSet<Entity> {
                EntityHashSet::default()
            }
        }

        impl Message for Visibility {}
    }
}
