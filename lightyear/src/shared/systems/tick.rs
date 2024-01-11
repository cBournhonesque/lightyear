use bevy::prelude::ResMut;
use tracing::trace;

use crate::shared::tick_manager::TickManaged;

pub fn increment_tick<T: TickManaged>(mut service: ResMut<T>) {
    service.increment_tick();
    trace!("increment_tick! new tick: {:?}", service.tick());
}
