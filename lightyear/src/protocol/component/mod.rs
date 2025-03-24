use crate::protocol::registry::{NetId, TypeKind};
use crate::serialize::SerializationError;
use bevy::prelude::Reflect;
use core::any::TypeId;
#[cfg(not(feature = "std"))]
use {
    alloc::{string::String},
};

pub(crate) mod delta;

pub(crate) mod interpolation;

pub(crate) mod prediction;

pub(crate) mod registry;
pub(crate) mod replication;

pub type ComponentNetId = NetId;

#[derive(thiserror::Error, Debug)]
pub enum ComponentError {
    #[error("component is not registered in the protocol")]
    NotRegistered,
    #[error("missing replication functions for component")]
    MissingReplicationFns,
    #[error("missing serialization functions for component")]
    MissingSerializationFns,
    #[error("missing delta compression functions for component")]
    MissingDeltaFns,
    #[error("delta compression error: {0}")]
    DeltaCompressionError(String),
    #[error("component error: {0}")]
    SerializationError(#[from] SerializationError),
}

/// [`ComponentKind`] is an internal wrapper around the type of the component
#[derive(Debug, Eq, Hash, Copy, Clone, PartialEq, Reflect)]
pub struct ComponentKind(pub TypeId);

impl ComponentKind {
    pub fn of<C: 'static>() -> Self {
        Self(TypeId::of::<C>())
    }
}

impl TypeKind for ComponentKind {}

impl From<TypeId> for ComponentKind {
    fn from(type_id: TypeId) -> Self {
        Self(type_id)
    }
}
