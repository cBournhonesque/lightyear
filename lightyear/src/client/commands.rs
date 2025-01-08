use bevy::prelude::Commands;
use std::ops::{Deref, DerefMut};

pub struct ClientCommands<'w, 's>(Commands<'w, 's>);

impl Deref for ClientCommands<'_, '_> {
    type Target = Commands<'_, '_>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl DerefMut for ClientCommands<'_, '_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

pub trait ClientCommandExt {
    fn client(self) -> ClientCommands;
}

impl ClientCommandExt for Commands {
    fn client(self) -> ClientCommands {
        ClientCommands(self)
    }
}
