//! Name of a struct or type

// TODO: replace with bevy TypePath? that would mean that we:
// - derive Reflect for Messages
// - require to derive Reflect for Components ?

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
