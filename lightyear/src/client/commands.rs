use bevy::ecs::system::SystemParam;
use bevy::prelude::Commands;
use std::ops::{Deref, DerefMut};

#[derive(SystemParam)]
pub struct ClientCommands<'w, 's>(Commands<'w, 's>);

impl<'w, 's> Deref for ClientCommands<'w, 's> {
    type Target = Commands<'w, 's>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl DerefMut for ClientCommands<'_, '_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

pub trait GetClientCommandsExt<'w, 's> {
    fn client(self) -> ClientCommands<'w, 's>;
}

impl<'w, 's> GetClientCommandsExt<'w, 's> for Commands<'w, 's> {
    fn client(self) -> ClientCommands<'w, 's> {
        ClientCommands(self)
    }
}
