//! Name of a struct or type

// TODO: replace with bevy TypePath? that would mean that we:
// - derive Reflect for Messages
// - require to derive Reflect for Components ?

use std::fmt::Debug;

pub trait TypeNamed {
    fn name() -> &'static str;
}

pub trait Named {
    // const NAME: &'static str;
    fn name(&self) -> &'static str;
}
