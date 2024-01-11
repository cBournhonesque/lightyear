//! Implement lightyear traits for some common bevy types
use crate::_reexport::LinearInterpolator;
use crate::client::components::{ComponentSyncMode, LerpFn, SyncComponent};
use bevy::prelude::{Entity, Transform};
use bevy::utils::EntityHashSet;
use std::ops::Mul;
use tracing::{info, trace};

use crate::prelude::{EntityMapper, MapEntities, Message, Named};

impl Named for Transform {
    const NAME: &'static str = "Transform";
}

pub struct TransformLinearInterpolation;

impl LerpFn<Transform> for TransformLinearInterpolation {
    fn lerp(start: Transform, other: Transform, t: f32) -> Transform {
        let translation = start.translation * (1.0 - t) + other.translation * t;
        let rotation = start.rotation.lerp(other.rotation, t);
        let scale = start.scale * (1.0 - t) + other.scale * t;
        let res = Transform {
            translation,
            rotation,
            scale,
        };
        trace!(
            "position lerp: start: {:?} end: {:?} t: {} res: {:?}",
            start,
            other,
            t,
            res
        );
        res
    }
}

impl<'a> MapEntities<'a> for Transform {
    fn map_entities(&mut self, entity_mapper: Box<dyn EntityMapper + 'a>) {}

    fn entities(&self) -> EntityHashSet<Entity> {
        EntityHashSet::default()
    }
}

cfg_if::cfg_if! {
    if #[cfg(feature = "render")] {
        use bevy::prelude::{Color,  Visibility};
        impl Named for Color {
            const NAME: &'static str = "Color";
        }

        impl<'a> MapEntities<'a> for Color {
            fn map_entities(&mut self, entity_mapper: Box<dyn EntityMapper + 'a>) {}

            fn entities(&self) -> EntityHashSet<Entity> {
                EntityHashSet::default()
            }
        }


        impl Named for Visibility {
            const NAME: &'static str = "Visibility";
        }

        impl<'a> MapEntities<'a> for Visibility {
            fn map_entities(&mut self, entity_mapper: Box<dyn EntityMapper + 'a>) {}

            fn entities(&self) -> EntityHashSet<Entity> {
                EntityHashSet::default()
            }
        }

    }
}
