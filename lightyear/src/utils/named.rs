// TODO: replace with bevy TypePath? that would mean that we:
// - derive Reflect for Messages
// - require to derive Reflect for Components ?

pub trait TypeNamed {
    fn name() -> String;
}

pub trait Named {
    fn name(&self) -> String;
}
