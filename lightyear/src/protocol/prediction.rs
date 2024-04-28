use crate::client::components::ComponentSyncMode;
use crate::prelude::client::SyncComponent;
use crate::protocol::serialize::SerializeFns;
use bevy::prelude::{Resource, TypePath};
use std::any::TypeId;

type LerpFn<C> = fn(start: &C, other: &C, t: f32) -> C;

#[derive(Debug, Default, Clone, Resource, PartialEq, TypePath)]
pub struct ErasedPredictionMetadata {
    pub prediction_mode: ComponentSyncMode,
    pub correction: Option<unsafe fn()>,
}

pub struct PredictionMetadata<C: SyncComponent> {
    pub prediction_mode: ComponentSyncMode,
    pub correction: LerpFn<C>,
}

impl ErasedPredictionMetadata {
    pub(crate) unsafe fn typed<C: SyncComponent>(&self) -> PredictionMetadata<C> {
        debug_assert_eq!(
            self.type_id,
            TypeId::of::<C>(),
            "The erased message fns were created for type {}, but we are trying to convert to type {}",
            self.type_name,
            std::any::type_name::<M>(),
        );

        SerializeFns {
            serialize: unsafe { std::mem::transmute(self.serialize) },
            deserialize: unsafe { std::mem::transmute(self.deserialize) },
            map_entities: self.map_entities.map(|m| unsafe { std::mem::transmute(m) }),
        }
    }
}
