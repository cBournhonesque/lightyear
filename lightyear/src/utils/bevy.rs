//! Implement lightyear traits for some common bevy types
use crate::_reexport::{InterpolatedComponent, LinearInterpolation};
use crate::client::components::{ComponentSyncMode, SyncComponent};
use bevy::prelude::{Entity, Transform};
use bevy::utils::EntityHashSet;
use std::ops::Mul;

use crate::prelude::{EntityMapper, MapEntities, Message, Named};

impl Named for Transform {
    fn name(&self) -> &'static str {
        "Transform"
    }
}

// impl SyncComponent for Transform {
//     fn mode() -> ComponentSyncMode {
//         ComponentSyncMode::Full
//     }
// }
//
// impl InterpolatedComponent<Transform> for Transform {
//     type Fn = Custom;
// }

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
