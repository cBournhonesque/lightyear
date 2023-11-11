use crate::tick::TickManaged;
use bevy::prelude::ResMut;

pub fn increment_tick<T: TickManaged>(mut service: ResMut<T>) {
    service.increment_tick();
}
