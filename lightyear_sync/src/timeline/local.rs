use crate::prelude::Timeline;
use crate::timeline::NetworkTimeline;
use bevy::prelude::{Query, Res, Time};
use bevy::time::Fixed;

/// The local timeline that matches Time<Virtual>
/// - the Tick is incremented every FixedUpdate
/// - the overstep is set by the overstep of Time<Fixed>
#[derive(Default)]
pub struct Local;

pub type LocalTimeline = Timeline<Local>;


/// Increment the local tick at each FixedUpdate
pub(crate) fn increment_local_tick(
    mut query: Query<&mut Timeline<Local>>,
) {
    query.iter_mut().for_each(|mut t| {
        let duration = t.tick_duration();
        t.advance(duration);
    })
}

/// Update the overstep using the Time<Fixed> overstep
pub(crate) fn set_local_overstep(
    fixed_time: Res<Time<Fixed>>,
    mut query: Query<&mut Timeline<Local>>,
) {
    let overstep = fixed_time.overstep();
    query.iter_mut().for_each(|mut t| {
        t.advance(overstep);
    })
}




