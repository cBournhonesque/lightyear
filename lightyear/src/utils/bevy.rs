//! Implement lightyear traits for some common bevy types
use bevy::prelude::{Entity, Transform};
use bevy::utils::EntityHashSet;

use crate::prelude::{EntityMapper, MapEntities, Message, Named};

impl Named for Transform {
    fn name(&self) -> String {
        "Transform".to_string()
    }
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
            fn name(&self) -> String {
                "Color".to_string()
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
            fn name(&self) -> String {
                "Visibility".to_string()
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
