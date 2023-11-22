use crate::client::interpolation::interpolation_history::ComponentHistory;
use crate::client::interpolation::{Interpolated, InterpolatedComponent};
use crate::client::prediction::Confirmed;
use crate::tick::Tick;
use crate::{Client, Protocol};
use bevy::prelude::{Component, Entity, Query, Res};
use tracing::warn;

#[derive(Component)]
pub struct InterpolateStatus<C: InterpolatedComponent> {
    /// start tick to interpolate from, along with value
    pub(crate) start: Option<(Tick, C)>,
    /// end tick to interpolate to, along with value
    pub(crate) end: Option<(Tick, C)>,
    /// current interpolation tick
    pub(crate) current: Tick,
}

/// At the end of each frame, interpolate the components between the last 2 confirmed server states
pub(crate) fn update_interpolate_status<C: InterpolatedComponent, P: Protocol>(
    client: Res<Client<P>>,
    mut query: Query<(&mut InterpolateStatus<C>, &mut ComponentHistory<C>)>,
) {
    let latest_received_server_tick = client.latest_received_server_tick();
    let current_interpolate_tick = client.interpolated_tick();

    if latest_received_server_tick < current_interpolate_tick {
        // the interpolated tick is ahead of the latest received server tick, so we can't interpolate
        warn!("Interpolated tick ({:?}) is ahead of the latest received server tick {:?}, so we can't interpolate",
            current_interpolate_tick, latest_received_server_tick);
        return;
    }

    // find the two confirmed states to interpolate between

    // start:
    // no S, no E: find S by popping until tick I (I included), and then pop again to get E (there could be no E also)
    //   - if no S or no E: set E or S, but do not interpolate

    // S < I < E: we can interpolate, do nothing
    // S < I = E: set S = E, and look for the next E
    for (mut status, mut history) in query.iter_mut() {
        let mut start = status.start.take();
        let mut end = status.end.take();

        // clear all values with a tick <= current_interpolate_tick, and get the last cleared value
        // (we need to call this even if status.start is set, because a new more recent server update could have been received)
        let new_start = history.pop_until_tick(current_interpolate_tick);
        if let Some((new_tick, _)) = new_start {
            if start.as_ref().map_or(true, |(tick, _)| *tick < new_tick) {
                start = new_start;
            }
        } else {
            // we didn't receive new events, but if we reach the end of interpolation, we need to replace start with end
            if let Some((end_tick, _)) = end {
                if end_tick == current_interpolate_tick {
                    start = end.clone();
                }
            }
        }

        // get the next value immediately > current_interpolate_tick
        // (we need to call this even if status.end is set, because a new more recent server update could have been received)
        let new_end = history.pop_next();
        if let Some((new_tick, _)) = new_end {
            if end.as_ref().map_or(true, |(tick, _)| new_tick < *tick) {
                end = new_end;
            }
        }

        if status.start.is_none() {
            warn!("no lerp start tick");
        }
        if status.end.is_none() {
            warn!("no lerp end tick");
        }
        status.start = start;
        status.end = end;
        status.current = current_interpolate_tick;
    }
}

pub(crate) fn interpolate<C: InterpolatedComponent>(
    mut query: Query<(&mut C, &InterpolateStatus<C>)>,
) {
    for (mut component, status) in query.iter_mut() {
        // only interpolate if we have both a start and an end
        // otherwise we keep our current value
        if let Some((start_tick, start_value)) = &status.start {
            if let Some((end_tick, end_value)) = &status.end {
                if start_tick != end_tick {
                    let t =
                        (status.current - *start_tick) as f32 / (*end_tick - *start_tick) as f32;
                    *component = C::lerp(start_value.clone(), end_value.clone(), t);
                } else {
                    *component = start_value.clone();
                }
            }
        }
    }
}
