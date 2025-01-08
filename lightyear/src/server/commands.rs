use bevy::ecs::system::SystemParam;
use bevy::prelude::Commands;
use std::ops::{Deref, DerefMut};

#[derive(SystemParam)]
pub struct ServerCommands<'w, 's>(Commands<'w, 's>);

impl<'w, 's> Deref for ServerCommands<'w, 's> {
    type Target = Commands<'w, 's>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl<'w, 's> DerefMut for ServerCommands<'w, 's> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

pub trait GetServerCommandsExt<'w, 's> {
    fn server(self) -> ServerCommands<'w, 's>;
}

impl<'w, 's> GetServerCommandsExt<'w, 's> for Commands<'w, 's> {
    fn server(self) -> ServerCommands<'w, 's> {
        ServerCommands(self)
    }
}
