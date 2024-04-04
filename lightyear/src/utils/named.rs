//! This module provides a trait `Named` to get the name of a type.
//!
//! We require that every `Message` that is part of the protocol implements `Named`.
//! (so that we can use the name for metrics, debugging, etc.)
use bevy::prelude::TypePath;

pub trait Named {
    const NAME: &'static str;
    fn type_name() -> &'static str {
        Self::NAME
    }
    fn name(&self) -> &'static str {
        Self::NAME
    }
}

/// A trait for structs who can get the name for another type.
/// This is used to avoid the orphan rule, as we can't implement [`Named`] on external types.
pub trait ExternalNamer<C> {
    fn type_name_for() -> &'static str;

    fn name_for(external: &C) -> &'static str;
}

/// We will implement `ExternalNamer` for all types that implement `TypePath`,
impl<T, C: TypePath> ExternalNamer<C> for T {
    fn type_name_for() -> &'static str {
        C::short_type_path()
    }

    fn name_for(external: &C) -> &'static str {
        C::short_type_path()
    }
}
