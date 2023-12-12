//! Implement lightyear traits for some common bevy types
use crate::prelude::{EntityMap, MapEntities, Message, Named};
use bevy::prelude::Transform;

impl Named for Transform {
    fn name(&self) -> String {
        "Transform".to_string()
    }
}

impl MapEntities for Transform {
    fn map_entities(&mut self, entity_map: &EntityMap) {}
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

        impl MapEntities for Color {
            fn map_entities(&mut self, entity_map: &EntityMap) {}
        }

        impl Message for Color {}

        impl Named for Visibility {
            fn name(&self) -> String {
                "Visibility".to_string()
            }
        }

        impl MapEntities for Visibility {
            fn map_entities(&mut self, entity_map: &EntityMap) {}
        }

        impl Message for Visibility {}
    }
}
