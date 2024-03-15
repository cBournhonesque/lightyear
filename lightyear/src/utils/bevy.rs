//! Implement lightyear traits for some common bevy types
use crate::_reexport::LinearInterpolator;
use crate::client::components::{ComponentSyncMode, LerpFn, SyncComponent};
use bevy::ecs::entity::{EntityHashSet, MapEntities};
use bevy::hierarchy::Parent;
use bevy::prelude::{Children, Entity, EntityMapper, Transform};
use std::ops::Mul;
use tracing::{info, trace};

use crate::prelude::{LightyearMapEntities, Message};

pub struct TransformLinearInterpolation;

impl LerpFn<Transform> for TransformLinearInterpolation {
    fn lerp(start: &Transform, other: &Transform, t: f32) -> Transform {
        let translation = start.translation * (1.0 - t) + other.translation * t;
        let rotation = start.rotation.slerp(other.rotation, t);
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

impl LightyearMapEntities for Transform {
    fn map_entities<M: EntityMapper>(&mut self, entity_mapper: &mut M) {}
}

cfg_if::cfg_if! {
    if #[cfg(feature = "render")] {
        use bevy::prelude::{Color,  Visibility};

        impl LightyearMapEntities for Color {
            fn map_entities<M: EntityMapper>(&mut self, entity_mapper: &mut M) {}
        }

        impl LightyearMapEntities for Visibility {
            fn map_entities<M: EntityMapper>(&mut self, entity_mapper: &mut M) {}
        }
    }
}
