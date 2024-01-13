use crate::_reexport::TickManager;
use bevy::prelude::ResMut;
use tracing::trace;

pub fn increment_tick(mut tick_manager: ResMut<TickManager>) {
    tick_manager.increment_tick();
    trace!("increment_tick! new tick: {:?}", tick_manager.tick());
}
